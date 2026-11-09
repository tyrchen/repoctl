//! Attribution of repository-root Cargo workspace changes to the projects whose crates they reach.
//!
//! A root `Cargo.lock` records every member's version, so it changes whenever any project bumps a
//! crate. Treating it as a repo-wide change would select every project for every Rust change.
//! Instead, the lock is diffed per package and the change is followed through reverse
//! dependencies to the members it reaches; a root `Cargo.toml` change is attributed through the
//! `[workspace.dependencies]` entries members inherit and the members added or removed. Anything
//! that cannot be attributed falls back to the caller's repo-wide handling.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
};

use globset::Glob;
use repoctl_core::{ProjectManifest, ProjectName, RepoRelativePath};
use serde_json::{Map, Value};
use toml_edit::{DocumentMut, Item, Table};

/// Repository-relative path of the root workspace manifest.
pub(super) const ROOT_MANIFEST: &str = "Cargo.toml";
/// Repository-relative path of the root workspace lockfile.
pub(super) const ROOT_LOCK: &str = "Cargo.lock";

const DEPENDENCY_TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

/// Text of a file before and after a change; `None` when the file does not exist on that side.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileVersions {
    /// Content before the change.
    pub old: Option<String>,
    /// Content after the change.
    pub new: Option<String>,
}

/// Projects reached by a change to the root `Cargo.toml` or `Cargo.lock`.
///
/// Returns `None` when the change cannot be attributed: the file is not part of a root workspace on
/// both sides, a file does not parse, a settings change every member inherits, or a lock entry
/// without a source that is not a workspace member.
pub(super) fn affected_projects(
    root: &Path,
    projects: &[ProjectManifest],
    file: &str,
    versions: &FileVersions,
) -> Option<BTreeSet<ProjectName>> {
    let members = Members::read(root)?;
    let directories = match file {
        ROOT_LOCK => lock_change(&members, versions)?,
        ROOT_MANIFEST => manifest_change(&members, versions)?,
        _ => return None,
    };
    Some(owning_projects(projects, &directories))
}

/// Workspace members of the current root manifest: package name to member directory.
struct Members {
    directories: BTreeMap<String, String>,
    manifests: BTreeMap<String, DocumentMut>,
}

impl Members {
    fn read(root: &Path) -> Option<Self> {
        let manifest = read_document(&root.join(ROOT_MANIFEST))?;
        let workspace = manifest.get("workspace")?.as_table()?;
        let mut directories = BTreeMap::new();
        let mut manifests = BTreeMap::new();
        for pattern in strings(workspace.get("members")) {
            for directory in expand_member(root, &pattern)? {
                let document = read_document(&root.join(&directory).join(ROOT_MANIFEST))?;
                let name = document
                    .get("package")
                    .and_then(|package| package.get("name"))
                    .and_then(Item::as_str)?
                    .to_string();
                directories.insert(name, directory.clone());
                manifests.insert(directory, document);
            }
        }
        Some(Self {
            directories,
            manifests,
        })
    }

    /// Members that inherit `[workspace.dependencies]` entry `name`.
    fn inheriting(&self, name: &str) -> BTreeSet<String> {
        self.manifests
            .iter()
            .filter(|(_, manifest)| inherits(manifest.as_table(), name))
            .map(|(directory, _)| directory.clone())
            .collect()
    }
}

fn inherits(manifest: &Table, name: &str) -> bool {
    let in_tables = |table: &Table| {
        DEPENDENCY_TABLES.iter().any(|section| {
            table
                .get(section)
                .and_then(Item::as_table_like)
                .and_then(|dependencies| dependencies.get(name))
                .and_then(Item::as_table_like)
                .and_then(|dependency| dependency.get("workspace"))
                .and_then(Item::as_bool)
                == Some(true)
        })
    };
    in_tables(manifest)
        || manifest
            .get("target")
            .and_then(Item::as_table_like)
            .is_some_and(|targets| {
                targets
                    .iter()
                    .filter_map(|(_, target)| target.as_table())
                    .any(in_tables)
            })
}

/// Member directories reached by a lockfile change.
fn lock_change(members: &Members, versions: &FileVersions) -> Option<BTreeSet<String>> {
    let old = Lock::parse(versions.old.as_deref()?)?;
    let new = Lock::parse(versions.new.as_deref()?)?;
    let changed: Vec<&Package> = old
        .entries
        .keys()
        .chain(new.entries.keys())
        .filter(|package| old.entries.get(*package) != new.entries.get(*package))
        .collect();
    let mut reached = BTreeSet::new();
    let mut queue: VecDeque<&Package> = changed.into_iter().collect();
    while let Some(package) = queue.pop_front() {
        if !reached.insert(package.clone()) {
            continue;
        }
        for lock in [&old, &new] {
            queue.extend(lock.dependents.get(package).into_iter().flatten());
        }
    }
    reached
        .iter()
        .filter(|package| package.source.is_none())
        .map(|package| members.directories.get(&package.name).cloned())
        .collect()
}

/// Member directories reached by a root manifest change.
fn manifest_change(members: &Members, versions: &FileVersions) -> Option<BTreeSet<String>> {
    let old = versions.old.as_deref()?.parse::<DocumentMut>().ok()?;
    let new = versions.new.as_deref()?.parse::<DocumentMut>().ok()?;
    if without_attributable(&old) != without_attributable(&new) {
        return None;
    }
    let dependencies = |document: &DocumentMut| {
        document
            .get("workspace")
            .and_then(|workspace| workspace.get("dependencies"))
            .and_then(Item::as_table_like)
            .map(|table| {
                table
                    .iter()
                    .map(|(name, item)| (name.to_string(), to_json(item)))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default()
    };
    let (old_dependencies, new_dependencies) = (dependencies(&old), dependencies(&new));
    let mut directories = BTreeSet::new();
    for name in old_dependencies.keys().chain(new_dependencies.keys()) {
        if old_dependencies.get(name) != new_dependencies.get(name) {
            directories.extend(members.inheriting(name));
        }
    }
    let member_patterns = |document: &DocumentMut| {
        strings(
            document
                .get("workspace")
                .and_then(|workspace| workspace.get("members")),
        )
        .into_iter()
        .collect::<BTreeSet<_>>()
    };
    directories.extend(
        member_patterns(&old)
            .symmetric_difference(&member_patterns(&new))
            .cloned(),
    );
    Some(directories)
}

/// The manifest as JSON without the parts whose changes are attributed to members.
fn without_attributable(document: &DocumentMut) -> Value {
    let mut value = to_json(document.as_item());
    if let Some(workspace) = value.get_mut("workspace").and_then(Value::as_object_mut) {
        workspace.remove("dependencies");
        workspace.remove("members");
    }
    value
}

/// A lockfile package identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Package {
    name: String,
    version: String,
    source: Option<String>,
}

/// Lockfile packages with their full entries and reverse dependency edges.
struct Lock {
    entries: BTreeMap<Package, Value>,
    dependents: BTreeMap<Package, BTreeSet<Package>>,
}

impl Lock {
    fn parse(text: &str) -> Option<Self> {
        let document = text.parse::<DocumentMut>().ok()?;
        let tables = document.get("package")?.as_array_of_tables()?;
        let mut entries = BTreeMap::new();
        let mut dependencies = Vec::new();
        for table in tables {
            let package = Package {
                name: table.get("name")?.as_str()?.to_string(),
                version: table.get("version")?.as_str()?.to_string(),
                source: table
                    .get("source")
                    .and_then(Item::as_str)
                    .map(str::to_string),
            };
            dependencies.push((package.clone(), strings(table.get("dependencies"))));
            entries.insert(package, to_json(&Item::Table(table.clone())));
        }
        let mut dependents: BTreeMap<Package, BTreeSet<Package>> = BTreeMap::new();
        for (package, references) in dependencies {
            for reference in references {
                let dependency = resolve(&entries, &reference)?;
                dependents
                    .entry(dependency)
                    .or_default()
                    .insert(package.clone());
            }
        }
        Some(Self {
            entries,
            dependents,
        })
    }
}

/// Resolves a lockfile dependency reference: `name`, `name version` or `name version (source)`.
fn resolve(entries: &BTreeMap<Package, Value>, reference: &str) -> Option<Package> {
    let mut parts = reference.splitn(3, ' ');
    let name = parts.next()?;
    let version = parts.next();
    let source = parts
        .next()
        .and_then(|source| source.strip_prefix('('))
        .and_then(|source| source.strip_suffix(')'));
    let mut candidates = entries.keys().filter(|package| {
        package.name == name
            && version.is_none_or(|version| package.version == version)
            && source.is_none_or(|source| package.source.as_deref() == Some(source))
    });
    let package = candidates.next()?;
    candidates.next().is_none().then(|| package.clone())
}

/// Member directories matching a `[workspace] members` entry; glob entries match directories that
/// contain a `Cargo.toml`.
fn expand_member(root: &Path, pattern: &str) -> Option<Vec<String>> {
    let pattern = pattern.trim_end_matches('/');
    if !pattern.contains(['*', '?', '[']) {
        return Some(vec![pattern.to_string()]);
    }
    let matcher = Glob::new(pattern).ok()?.compile_matcher();
    let base = pattern
        .split('/')
        .take_while(|component| !component.contains(['*', '?', '[']))
        .collect::<Vec<_>>()
        .join("/");
    let mut directories = Vec::new();
    for entry in ignore::WalkBuilder::new(root.join(&base))
        .hidden(false)
        .build()
        .flatten()
    {
        let path = entry.path();
        if entry.file_type().is_some_and(|kind| kind.is_dir())
            && path.join(ROOT_MANIFEST).is_file()
            && let Ok(relative) = path.strip_prefix(root)
            && let Some(relative) = relative.to_str()
            && matcher.is_match(relative)
        {
            directories.push(relative.to_string());
        }
    }
    directories.sort();
    Some(directories)
}

fn owning_projects(
    projects: &[ProjectManifest],
    directories: &BTreeSet<String>,
) -> BTreeSet<ProjectName> {
    directories
        .iter()
        .filter_map(|directory| RepoRelativePath::new(format!("{directory}/{ROOT_MANIFEST}")).ok())
        .flat_map(|manifest| {
            projects
                .iter()
                .filter(move |project| project.contains_path(&manifest))
                .map(|project| project.name.clone())
        })
        .collect()
}

fn read_document(path: &Path) -> Option<DocumentMut> {
    std::fs::read_to_string(path).ok()?.parse().ok()
}

fn strings(item: Option<&Item>) -> Vec<String> {
    item.and_then(Item::as_array)
        .map(|array| {
            array
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A formatting-independent value of a TOML item, so comments and layout never count as changes.
fn to_json(item: &Item) -> Value {
    match item {
        Item::None => Value::Null,
        Item::Value(value) => value_to_json(value),
        Item::Table(table) => table_to_json(table.iter()),
        Item::ArrayOfTables(array) => Value::Array(
            array
                .iter()
                .map(|table| table_to_json(table.iter()))
                .collect(),
        ),
    }
}

fn table_to_json<'a>(entries: impl Iterator<Item = (&'a str, &'a Item)>) -> Value {
    Value::Object(
        entries
            .map(|(key, item)| (key.to_string(), to_json(item)))
            .collect::<Map<_, _>>(),
    )
}

fn value_to_json(value: &toml_edit::Value) -> Value {
    match value {
        toml_edit::Value::String(text) => Value::String(text.value().clone()),
        toml_edit::Value::Integer(number) => Value::from(*number.value()),
        toml_edit::Value::Float(number) => Value::from(*number.value()),
        toml_edit::Value::Boolean(flag) => Value::Bool(*flag.value()),
        toml_edit::Value::Datetime(datetime) => Value::String(datetime.value().to_string()),
        toml_edit::Value::Array(array) => Value::Array(array.iter().map(value_to_json).collect()),
        toml_edit::Value::InlineTable(table) => Value::Object(
            table
                .iter()
                .map(|(key, value)| (key.to_string(), value_to_json(value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = r#"version = 4

[[package]]
name = "alpha"
version = "0.1.0"
dependencies = ["serde"]

[[package]]
name = "beta"
version = "0.2.0"
dependencies = ["alpha", "tokio"]

[[package]]
name = "gamma"
version = "1.0.0"
dependencies = ["tokio"]

[[package]]
name = "serde"
version = "1.0.200"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "tokio"
version = "1.48.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;

    const MANIFEST: &str = r#"[workspace]
resolver = "3"
members = ["apps/one/alpha", "apps/two/beta", "tools/gamma"]

[workspace.dependencies]
serde = "1"
tokio = { version = "1", features = ["rt"] }
"#;

    fn members() -> Members {
        let manifest = |name: &str, dependencies: &str| -> DocumentMut {
            format!("[package]\nname = \"{name}\"\n\n[dependencies]\n{dependencies}")
                .parse()
                .unwrap()
        };
        let entries = [
            ("alpha", "apps/one/alpha", "serde.workspace = true\n"),
            (
                "beta",
                "apps/two/beta",
                "alpha = { path = \"../../one/alpha\" }\ntokio = { workspace = true, features = \
                 [\"net\"] }\n",
            ),
            ("gamma", "tools/gamma", "tokio = \"1\"\n"),
        ];
        Members {
            directories: entries
                .iter()
                .map(|(name, directory, _)| ((*name).to_string(), (*directory).to_string()))
                .collect(),
            manifests: entries
                .iter()
                .map(|(name, directory, dependencies)| {
                    ((*directory).to_string(), manifest(name, dependencies))
                })
                .collect(),
        }
    }

    fn versions(old: &str, new: &str) -> FileVersions {
        FileVersions {
            old: Some(old.to_string()),
            new: Some(new.to_string()),
        }
    }

    fn set(items: &[&str]) -> Option<BTreeSet<String>> {
        Some(items.iter().map(|item| (*item).to_string()).collect())
    }

    #[test]
    fn test_should_attribute_a_member_version_bump_to_that_member_and_its_dependents() {
        let bumped = LOCK.replace(
            "name = \"alpha\"\nversion = \"0.1.0\"",
            "name = \"alpha\"\nversion = \"0.1.1\"",
        );
        assert_eq!(
            lock_change(&members(), &versions(LOCK, &bumped)),
            set(&["apps/one/alpha", "apps/two/beta"])
        );
    }

    #[test]
    fn test_should_attribute_a_registry_update_to_the_members_that_reach_it() {
        let updated = LOCK.replace("1.0.200", "1.0.201");
        assert_eq!(
            lock_change(&members(), &versions(LOCK, &updated)),
            set(&["apps/one/alpha", "apps/two/beta"])
        );
        let updated = LOCK.replace("1.48.0", "1.49.0");
        assert_eq!(
            lock_change(&members(), &versions(LOCK, &updated)),
            set(&["apps/two/beta", "tools/gamma"])
        );
    }

    #[test]
    fn test_should_ignore_lock_formatting_and_refuse_unknown_path_packages() {
        let reformatted = format!("# regenerated\n{LOCK}");
        assert_eq!(
            lock_change(&members(), &versions(LOCK, &reformatted)),
            set(&[])
        );
        let foreign = LOCK.replace("name = \"gamma\"", "name = \"delta\"");
        assert_eq!(lock_change(&members(), &versions(LOCK, &foreign)), None);
    }

    #[test]
    fn test_should_attribute_workspace_dependency_changes_to_inheriting_members() {
        let changed = MANIFEST.replace("features = [\"rt\"]", "features = [\"rt\", \"time\"]");
        assert_eq!(
            manifest_change(&members(), &versions(MANIFEST, &changed)),
            set(&["apps/two/beta"])
        );
        let added = MANIFEST.replace("\"tools/gamma\"]", "\"tools/gamma\", \"tools/delta\"]");
        assert_eq!(
            manifest_change(&members(), &versions(MANIFEST, &added)),
            set(&["tools/delta"])
        );
    }

    #[test]
    fn test_should_treat_other_root_manifest_changes_as_unattributable() {
        let profile = format!("{MANIFEST}\n[profile.release]\nlto = true\n");
        assert_eq!(
            manifest_change(&members(), &versions(MANIFEST, &profile)),
            None
        );
        let removed = FileVersions {
            old: Some(MANIFEST.to_string()),
            new: None,
        };
        assert_eq!(manifest_change(&members(), &removed), None);
    }
}
