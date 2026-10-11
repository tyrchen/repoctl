//! Package indexing and option-preserving local dependency rewrites.

use std::{collections::BTreeMap, fs};

use camino::Utf8Path;
use repoctl_core::{
    AdoptedSource, AdoptionFileOperation, DependencyRewrite, DependencySurface, Diagnostic,
    ProjectName, ProjectRelativePath, RepoRelativePath, RepoctlError,
};
use serde_json::Value;
use toml_edit::{DocumentMut, InlineTable, Item, Value as TomlValue, value};

use super::{packages::PackageIndex, synthesis::project_name_for_destination};

pub(super) fn plan_dependency_rewrites(
    sources: &[AdoptedSource],
    index: &PackageIndex,
) -> Result<Vec<DependencyRewrite>, RepoctlError> {
    let mut rewrites = Vec::new();
    for source in sources.iter().filter(|source| !source.skipped) {
        for manifest in &source.inventory.manifests {
            let file = source
                .destination_path
                .join_project(
                    &ProjectRelativePath::new(manifest).map_err(RepoctlError::diagnostic)?,
                )
                .map_err(RepoctlError::diagnostic)?;
            let content = fs::read_to_string(source.source_path.join(manifest))
                .map_err(|error| RepoctlError::Environment(error.to_string()))?;
            let owner_project =
                project_name_for_destination(&source.destination_path, &source.inferred_kind)?;
            if manifest.ends_with("Cargo.toml") {
                plan_cargo_rewrites(
                    source,
                    manifest,
                    &content,
                    &file,
                    &owner_project,
                    index,
                    &mut rewrites,
                )?;
            } else if manifest.ends_with("package.json") {
                plan_node_rewrites(&content, &file, &owner_project, index, &mut rewrites)?;
            }
        }
    }
    rewrites.sort_by(|a, b| (&a.file, &a.table, &a.package).cmp(&(&b.file, &b.table, &b.package)));
    Ok(rewrites)
}

fn plan_cargo_rewrites(
    source: &AdoptedSource,
    manifest: &str,
    content: &str,
    file: &RepoRelativePath,
    owner_project: &ProjectName,
    index: &PackageIndex,
    rewrites: &mut Vec<DependencyRewrite>,
) -> Result<(), RepoctlError> {
    let document = content
        .parse::<DocumentMut>()
        .map_err(|error| RepoctlError::Environment(error.to_string()))?;
    for table in cargo_dependency_tables(&document) {
        let Some(deps) = table_item(document.as_item(), &table).and_then(Item::as_table_like)
        else {
            continue;
        };
        for (alias, item) in deps.iter() {
            if item.get("workspace").and_then(Item::as_bool) == Some(true) {
                validate_workspace_inheritance(&source.source_path, manifest, alias)?;
                continue;
            }
            let package = item.get("package").and_then(Item::as_str).unwrap_or(alias);
            if let Some(target) = index.rust.get(package) {
                rewrites.push(DependencyRewrite {
                    table: table.clone(),
                    file: file.clone(),
                    package: alias.to_string(),
                    from: item.to_string(),
                    to: relative_dependency_path(file, target),
                    surface: DependencySurface::FrameworkFacade,
                    owner_project: owner_project.clone(),
                });
            }
        }
    }
    Ok(())
}

fn plan_node_rewrites(
    content: &str,
    file: &RepoRelativePath,
    owner_project: &ProjectName,
    index: &PackageIndex,
    rewrites: &mut Vec<DependencyRewrite>,
) -> Result<(), RepoctlError> {
    let document: Value = serde_json::from_str(content)
        .map_err(|error| RepoctlError::Environment(error.to_string()))?;
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(deps) = document.get(section).and_then(Value::as_object) {
            for (package, from) in deps {
                if let Some(target) = index.node.get(package) {
                    rewrites.push(DependencyRewrite {
                        table: vec![section.to_string()],
                        file: file.clone(),
                        package: package.clone(),
                        from: from.to_string(),
                        to: format!("file:{}", relative_dependency_path(file, target)),
                        surface: DependencySurface::FrameworkFacade,
                        owner_project: owner_project.clone(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn validate_workspace_inheritance(
    source: &Utf8Path,
    manifest: &str,
    alias: &str,
) -> Result<(), RepoctlError> {
    let mut directory = source.join(manifest).parent().map(Utf8Path::to_path_buf);
    while let Some(path) = directory {
        if !path.starts_with(source) {
            break;
        }
        let cargo = path.join("Cargo.toml");
        if cargo.is_file() {
            let text = fs::read_to_string(&cargo)
                .map_err(|error| RepoctlError::io(cargo.clone(), error))?;
            let document = text
                .parse::<DocumentMut>()
                .map_err(|error| RepoctlError::Environment(error.to_string()))?;
            if let Some(workspace) = document.get("workspace") {
                if workspace
                    .get("dependencies")
                    .and_then(|dependencies| dependencies.get(alias))
                    .is_some()
                {
                    return Ok(());
                }
                break;
            }
        }
        directory = path.parent().map(Utf8Path::to_path_buf);
    }
    Err(RepoctlError::diagnostic(Diagnostic::error(
        "adoption.workspace_dependency",
        format!(
            "cannot resolve inherited dependency `{alias}` in `{manifest}` inside its adopted \
             source"
        ),
    )))
}

fn cargo_dependency_tables(document: &DocumentMut) -> Vec<Vec<String>> {
    let mut tables = Vec::new();
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        tables.push(vec![section.to_string()]);
        if let Some(targets) = document.get("target").and_then(Item::as_table_like) {
            for (target, _) in targets.iter() {
                tables.push(vec![
                    "target".to_string(),
                    target.to_string(),
                    section.to_string(),
                ]);
            }
        }
    }
    tables.push(vec!["workspace".to_string(), "dependencies".to_string()]);
    tables
}

fn table_item<'a>(item: &'a Item, parts: &[String]) -> Option<&'a Item> {
    parts
        .iter()
        .try_fold(item, |current, part| current.get(part))
}

pub(super) fn apply_rewrites_to_operations(
    operations: &mut [AdoptionFileOperation],
    rewrites: &[DependencyRewrite],
) -> Result<(), RepoctlError> {
    let mut by_file: BTreeMap<RepoRelativePath, Vec<&DependencyRewrite>> = BTreeMap::new();
    for rewrite in rewrites {
        by_file
            .entry(rewrite.file.clone())
            .or_default()
            .push(rewrite);
    }
    for (file, file_rewrites) in by_file {
        let Some(operation) = operations
            .iter_mut()
            .find(|operation| operation.destination_path == file)
        else {
            continue;
        };
        let Some(source_path) = &operation.source_path else {
            continue;
        };
        let content = fs::read_to_string(source_path)
            .map_err(|source| RepoctlError::io(source_path.clone(), source))?;
        let rewritten = if file.as_str().ends_with("Cargo.toml") {
            rewrite_cargo_manifest(&content, &file_rewrites)?
        } else if file.as_str().ends_with("package.json") {
            rewrite_package_json(&content, &file_rewrites)?
        } else {
            content
        };
        operation.content = Some(rewritten);
    }
    Ok(())
}

fn rewrite_cargo_manifest(
    content: &str,
    rewrites: &[&DependencyRewrite],
) -> Result<String, RepoctlError> {
    let mut document = content
        .parse::<DocumentMut>()
        .map_err(|error| RepoctlError::Environment(error.to_string()))?;
    for rewrite in rewrites {
        let table = if rewrite.table.is_empty() {
            vec!["dependencies".to_string()]
        } else {
            rewrite.table.clone()
        };
        let mut current = document.as_item_mut();
        for part in &table {
            current = current.get_mut(part).ok_or_else(|| {
                RepoctlError::Environment(format!("dependency table `{part}` disappeared"))
            })?;
        }
        let item = current
            .get_mut(&rewrite.package)
            .ok_or_else(|| RepoctlError::Environment("dependency alias disappeared".to_string()))?;
        if item.is_str() {
            *item = Item::Value(TomlValue::InlineTable(InlineTable::default()));
        }
        let fields = item.as_table_like_mut().ok_or_else(|| {
            RepoctlError::Environment("dependency must be string or table".to_string())
        })?;
        for field in [
            "version",
            "git",
            "branch",
            "tag",
            "rev",
            "registry",
            "workspace",
        ] {
            fields.remove(field);
        }
        fields.insert("path", value(rewrite.to.clone()));
    }
    Ok(document.to_string())
}

fn rewrite_package_json(
    content: &str,
    rewrites: &[&DependencyRewrite],
) -> Result<String, RepoctlError> {
    let mut value = serde_json::from_str::<Value>(content).map_err(|source| {
        RepoctlError::diagnostic(Diagnostic::error(
            "adoption.dependency.rewrite",
            format!("failed to parse package.json: {source}"),
        ))
    })?;
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        let Some(object) = value.get_mut(section).and_then(Value::as_object_mut) else {
            continue;
        };
        for rewrite in rewrites {
            if (rewrite.table.is_empty()
                || rewrite.table.first().is_some_and(|table| table == section))
                && object.contains_key(&rewrite.package)
            {
                object.insert(rewrite.package.clone(), Value::String(rewrite.to.clone()));
            }
        }
    }
    serde_json::to_string_pretty(&value)
        .map(|mut output| {
            output.push('\n');
            output
        })
        .map_err(|source| {
            RepoctlError::Internal(format!("failed to render rewritten package.json: {source}"))
        })
}

fn relative_dependency_path(from_file: &RepoRelativePath, target: &RepoRelativePath) -> String {
    let from_parent = from_file
        .as_str()
        .rsplit_once('/')
        .map_or(".", |(parent, _)| parent);
    let from_parts = from_parent
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>();
    let target_parts = target
        .as_str()
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>();
    let common = from_parts
        .iter()
        .zip(target_parts.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts = Vec::new();
    for _ in common..from_parts.len() {
        parts.push("..".to_string());
    }
    parts.extend(
        target_parts[common..]
            .iter()
            .map(|part| (*part).to_string()),
    );
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}
