//! Bounded offline traversal of Cargo members and implicit in-workspace path packages.

use std::{
    collections::{BTreeSet, VecDeque},
    path::{Component, Path, PathBuf},
};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use repoctl_core::{DiscoveredEdge, RepoctlError, WorkspaceInspectionInput};
use toml_edit::{DocumentMut, Item};

use super::{
    append_path_edge, cargo_tables, inherited_cargo_dependency, manifest_error, parse_toml,
    read_manifest,
};

const MAX_MEMBERS: usize = 4096;
const MAX_ENTRIES: usize = 100_000;

pub(super) fn inspect(
    input: &WorkspaceInspectionInput<'_>,
    manifest: &Path,
    doc: &DocumentMut,
) -> Result<Vec<DiscoveredEdge>, RepoctlError> {
    let mut edges = Vec::new();
    walk(
        input,
        manifest,
        doc,
        false,
        &mut |_, _| Ok(()),
        &mut |origin, local, name| append_path_edge(input, origin, local, name, &mut edges),
    )?;
    Ok(edges)
}

pub(super) fn requires_protoc(
    input: &WorkspaceInspectionInput<'_>,
    manifest: &Path,
    doc: &DocumentMut,
) -> Result<bool, RepoctlError> {
    let mut required = false;
    walk(
        input,
        manifest,
        doc,
        true,
        &mut |path, document| {
            let text = document.to_string();
            required |= text.contains("prost-build")
                || text.contains("tonic-build")
                || text.contains("protobuf-codegen");
            if let Some(package) = document.get("package") {
                let build = package.get("build");
                if build.and_then(Item::as_bool) != Some(false) {
                    let script = build.and_then(Item::as_str).unwrap_or("build.rs");
                    if script.len() > 4096 || script.contains('\0') {
                        return Err(manifest_error(path, "invalid Cargo build script path"));
                    }
                    let parent = path
                        .parent()
                        .ok_or_else(|| manifest_error(path, "manifest has no parent"))?;
                    if let Some(text) =
                        read_manifest(&parent.join(script), input.root.absolute.as_std_path())?
                    {
                        required |= text.contains("protoc")
                            || text.contains("prost_build")
                            || text.contains("tonic_build");
                    }
                }
            }
            Ok(())
        },
        &mut |_, _, _| Ok(()),
    )?;
    Ok(required)
}

fn walk(
    input: &WorkspaceInspectionInput<'_>,
    manifest: &Path,
    doc: &DocumentMut,
    follow_external_paths: bool,
    on_manifest: &mut impl FnMut(&Path, &DocumentMut) -> Result<(), RepoctlError>,
    on_dependency: &mut impl FnMut(&Path, &str, &str) -> Result<(), RepoctlError>,
) -> Result<(), RepoctlError> {
    let directory = manifest
        .parent()
        .ok_or_else(|| manifest_error(manifest, "manifest has no parent"))?;
    let excluded = patterns(doc, "exclude", manifest)?;
    let dependency_excludes = if follow_external_paths {
        GlobSet::empty()
    } else {
        excluded.clone()
    };
    let dependency_root = if follow_external_paths {
        input.root.absolute.as_std_path()
    } else {
        directory
    };
    let mut queue = VecDeque::from([(manifest.to_path_buf(), doc.clone())]);
    for member in members(directory, manifest, doc, &excluded, input)? {
        queue.push_back((member.clone(), required_manifest(&member, input)?));
    }
    let mut visited = BTreeSet::new();
    while let Some((path, document)) = queue.pop_front() {
        if !visited.insert(path.clone()) {
            continue;
        }
        if visited.len() > MAX_MEMBERS {
            return Err(manifest_error(
                manifest,
                "Cargo workspace exceeds 4096 packages",
            ));
        }
        on_manifest(&path, &document)?;
        cargo_tables(&document, &mut |table| {
            for (name, item) in table.iter() {
                let dependency = if let Some(local) = item.get("path").and_then(Item::as_str) {
                    Some((path.clone(), local.to_string()))
                } else if item.get("workspace").and_then(Item::as_bool) == Some(true) {
                    inherited_cargo_dependency(&path, name, input)?
                } else {
                    None
                };
                if let Some((origin, local)) = dependency {
                    on_dependency(&origin, &local, name)?;
                    if follow_external_paths || doc.get("workspace").is_some() {
                        enqueue_path_package(
                            &origin,
                            &local,
                            dependency_root,
                            &dependency_excludes,
                            input,
                            &visited,
                            &mut queue,
                        )?;
                    }
                }
            }
            Ok(())
        })?;
    }
    Ok(())
}

fn enqueue_path_package(
    origin: &Path,
    local: &str,
    root: &Path,
    excluded: &GlobSet,
    input: &WorkspaceInspectionInput<'_>,
    visited: &BTreeSet<PathBuf>,
    queue: &mut VecDeque<(PathBuf, DocumentMut)>,
) -> Result<(), RepoctlError> {
    let Some(parent) = origin.parent() else {
        return Ok(());
    };
    let candidate = parent.join(local);
    if !candidate.exists() {
        return Ok(());
    }
    let candidate = candidate
        .canonicalize()
        .map_err(|error| RepoctlError::io(candidate.display().to_string(), error))?;
    let Ok(relative) = candidate.strip_prefix(root) else {
        return Ok(());
    };
    let manifest = candidate.join("Cargo.toml");
    if !excluded.is_match(relative)
        && !visited.contains(&manifest)
        && !queue.iter().any(|(queued, _)| queued == &manifest)
        && manifest.is_file()
    {
        if queue.len().saturating_add(visited.len()) >= MAX_MEMBERS {
            return Err(manifest_error(
                &manifest,
                "Cargo workspace exceeds 4096 packages",
            ));
        }
        queue.push_back((manifest.clone(), required_manifest(&manifest, input)?));
    }
    Ok(())
}

fn required_manifest(
    path: &Path,
    input: &WorkspaceInspectionInput<'_>,
) -> Result<DocumentMut, RepoctlError> {
    let text = read_manifest(path, input.root.absolute.as_std_path())?
        .ok_or_else(|| manifest_error(path, "Cargo member manifest is missing"))?;
    parse_toml(&text, path)
}

fn member_patterns<'a>(
    doc: &'a DocumentMut,
    key: &str,
    path: &Path,
) -> Result<Vec<&'a str>, RepoctlError> {
    let Some(value) = doc
        .get("workspace")
        .and_then(|workspace| workspace.get(key))
    else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| manifest_error(path, "Cargo workspace members/exclude must be arrays"))?;
    if array.len() > MAX_MEMBERS {
        return Err(manifest_error(path, "too many Cargo member patterns"));
    }
    array
        .iter()
        .map(|value| {
            let pattern = value
                .as_str()
                .ok_or_else(|| manifest_error(path, "Cargo member patterns must be strings"))?;
            if pattern.is_empty() || pattern.len() > 4096 || Path::new(pattern).is_absolute() {
                return Err(manifest_error(path, "invalid Cargo member pattern"));
            }
            Ok(pattern)
        })
        .collect()
}

fn patterns(doc: &DocumentMut, key: &str, path: &Path) -> Result<GlobSet, RepoctlError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in member_patterns(doc, key, path)? {
        builder.add(
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .map_err(|error| manifest_error(path, &error.to_string()))?,
        );
    }
    builder
        .build()
        .map_err(|error| manifest_error(path, &error.to_string()))
}

fn members(
    root: &Path,
    manifest: &Path,
    doc: &DocumentMut,
    excluded: &GlobSet,
    input: &WorkspaceInspectionInput<'_>,
) -> Result<BTreeSet<PathBuf>, RepoctlError> {
    let mut result = BTreeSet::new();
    let mut count = 0_usize;
    let patterns = member_patterns(doc, "members", manifest)?;
    if patterns.is_empty() {
        return Ok(result);
    }
    for pattern in patterns {
        result.extend(expand_member_pattern(
            root, pattern, manifest, excluded, input, &mut count,
        )?);
        if result.len() > MAX_MEMBERS {
            return Err(manifest_error(
                manifest,
                "Cargo workspace exceeds 4096 packages",
            ));
        }
    }
    Ok(result)
}

fn expand_member_pattern(
    root: &Path,
    pattern: &str,
    manifest: &Path,
    excluded: &GlobSet,
    input: &WorkspaceInspectionInput<'_>,
    count: &mut usize,
) -> Result<BTreeSet<PathBuf>, RepoctlError> {
    let mut result = BTreeSet::new();
    let prefix = pattern
        .split('/')
        .take_while(|part| !part.contains(['*', '?', '[', '{']))
        .collect::<PathBuf>();
    if prefix == Path::new(pattern) {
        if excluded.is_match(pattern) {
            return Ok(result);
        }
        let member = root.join(pattern).join("Cargo.toml");
        required_manifest(&member, input)?;
        result.insert(
            member
                .canonicalize()
                .map_err(|error| RepoctlError::io(member.display().to_string(), error))?,
        );
        return Ok(result);
    }
    let Some(base) = member_base(root, &prefix, manifest, input)? else {
        return Ok(result);
    };
    let suffix = Path::new(pattern)
        .strip_prefix(&prefix)
        .map_err(|error| manifest_error(manifest, &error.to_string()))?;
    let matcher = GlobBuilder::new(&base.join(suffix).to_string_lossy())
        .literal_separator(true)
        .build()
        .map_err(|error| manifest_error(manifest, &error.to_string()))?
        .compile_matcher();
    let walker = WalkBuilder::new(base)
        .standard_filters(false)
        .follow_links(false)
        .max_depth(Some(64))
        .filter_entry(|entry| !matches!(entry.file_name().to_str(), Some(".git" | "target")))
        .build();
    for entry in walker {
        if *count >= MAX_ENTRIES {
            return Err(manifest_error(
                manifest,
                "Cargo member traversal exceeds 100000 entries",
            ));
        }
        *count = count.saturating_add(1);
        let entry = entry.map_err(|error| manifest_error(manifest, &error.to_string()))?;
        if entry.depth() == 64 && entry.file_type().is_some_and(|kind| kind.is_dir()) {
            return Err(manifest_error(
                manifest,
                "Cargo member traversal exceeds 64 levels",
            ));
        }
        let directory = entry.path();
        if entry.file_type().is_some_and(|kind| kind.is_symlink()) && matcher.is_match(directory) {
            return Err(manifest_error(
                manifest,
                "Cargo member pattern matches a symlink",
            ));
        }
        if entry.file_type().is_some_and(|kind| kind.is_dir())
            && matcher.is_match(directory)
            && !excluded.is_match(directory.strip_prefix(root).unwrap_or(directory))
        {
            let member = directory.join("Cargo.toml");
            required_manifest(&member, input)?;
            result.insert(member);
            if result.len() > MAX_MEMBERS {
                return Err(manifest_error(
                    manifest,
                    "Cargo workspace exceeds 4096 packages",
                ));
            }
        }
    }
    Ok(result)
}

fn member_base(
    root: &Path,
    prefix: &Path,
    manifest: &Path,
    input: &WorkspaceInspectionInput<'_>,
) -> Result<Option<PathBuf>, RepoctlError> {
    let mut base = PathBuf::new();
    for part in root.join(prefix).components() {
        match part {
            Component::ParentDir => {
                base.pop();
            }
            Component::CurDir => {}
            _ => base.push(part.as_os_str()),
        }
    }
    if !base.starts_with(input.root.absolute.as_std_path()) {
        return Err(manifest_error(manifest, "Cargo members escape repository"));
    }
    if !base.exists() {
        return Ok(None);
    }
    let base = base
        .canonicalize()
        .map_err(|error| RepoctlError::io(base.display().to_string(), error))?;
    if !base.starts_with(input.root.absolute.as_std_path()) {
        return Err(manifest_error(manifest, "Cargo members escape repository"));
    }
    Ok(Some(base))
}
