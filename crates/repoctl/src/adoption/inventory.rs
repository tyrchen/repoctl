//! Local source inventory and dependency references.

use std::{collections::BTreeSet, fs};

use camino::{Utf8Path, Utf8PathBuf};
use repoctl_core::{Diagnostic, RepoctlError, SourceInventory};
use serde_json::Value;
use toml_edit::{DocumentMut, Item};

use super::source_walk::{
    MAX_SOURCES, ScanBudget, SourceEntry, SourceEntryKind, limit, read_text, walk_source,
};

pub(super) fn source_repositories(
    source_root: &Utf8Path,
    dest_root: &Utf8Path,
) -> Result<Vec<Utf8PathBuf>, RepoctlError> {
    if is_repository(source_root) {
        return Ok(vec![source_root.to_path_buf()]);
    }
    let mut repos = Vec::new();
    let mut budget = ScanBudget::default();
    let entries = fs::read_dir(source_root)
        .map_err(|source| RepoctlError::io(source_root.to_path_buf(), source))?;
    for entry in entries {
        let entry = entry.map_err(|source| RepoctlError::io(source_root.to_path_buf(), source))?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|path| {
            RepoctlError::diagnostic(Diagnostic::error(
                "path.non_utf8",
                format!("path is not valid UTF-8: {}", path.display()),
            ))
        })?;
        if dest_root.starts_with(source_root) && path.starts_with(dest_root) {
            continue;
        }
        budget.visit(&path, false, 1)?;
        if path.is_symlink() {
            return Err(RepoctlError::diagnostic(Diagnostic::error(
                "adoption.copy.symlink",
                format!("adoption source contains a symlink: {path}"),
            )));
        }
        if path.is_dir() && is_repository(&path) {
            if repos.len() >= MAX_SOURCES {
                return Err(limit("sources", &path));
            }
            repos.push(path);
        }
    }
    repos.sort();
    Ok(repos)
}

fn is_repository(path: &Utf8Path) -> bool {
    [
        "Cargo.toml",
        "pyproject.toml",
        "package.json",
        "project.yaml",
        "project.yml",
        "Pulumi.yaml",
        "README.md",
    ]
    .iter()
    .any(|marker| path.join(marker).is_file())
}

pub(super) fn source_name(path: &Utf8Path) -> Result<String, RepoctlError> {
    path.file_name().map(ToString::to_string).ok_or_else(|| {
        RepoctlError::diagnostic(Diagnostic::error(
            "adoption.source.name",
            "source path must have a directory name",
        ))
    })
}

#[derive(Debug, Default)]
struct InventoryScan {
    manifests: Vec<String>,
    top_level_dirs: Vec<String>,
    dependencies: BTreeSet<String>,
    generated: Vec<String>,
    tools: BTreeSet<String>,
}

pub(super) fn inventory_source(
    source_path: &Utf8Path,
    name: &str,
    destination: &Utf8Path,
    budget: &mut ScanBudget,
) -> Result<SourceInventory, RepoctlError> {
    let mut scan = InventoryScan::default();
    walk_source(source_path, destination, budget, &mut |entry| {
        inventory_entry(entry, &mut scan)
    })?;
    scan.top_level_dirs.sort();
    scan.manifests.sort();
    scan.generated.sort();
    Ok(SourceInventory {
        name: name.to_string(),
        has_vcs: source_path.join(".git").exists(),
        readme_summary: read_readme_summary(source_path)?,
        manifests: scan.manifests,
        top_level_dirs: scan.top_level_dirs,
        dependency_references: scan.dependencies.into_iter().collect(),
        generated_artifacts: scan.generated,
        required_tools: scan.tools.into_iter().collect(),
    })
}

fn inventory_entry(entry: SourceEntry, scan: &mut InventoryScan) -> Result<(), RepoctlError> {
    let relative = entry.relative.as_str();
    let Some(name) = entry.path.file_name() else {
        return Ok(());
    };
    if entry.kind == SourceEntryKind::Directory && !relative.contains('/') {
        scan.top_level_dirs.push(name.to_string());
    }
    if entry.kind == SourceEntryKind::Excluded {
        scan.generated.push(relative.to_string());
        return Ok(());
    }
    if entry.kind != SourceEntryKind::File {
        return Ok(());
    }
    if is_primary_manifest(name) {
        scan.manifests.push(relative.to_string());
    }
    match name {
        "Cargo.toml" => {
            collect_cargo_dependencies(&entry.path, &mut scan.dependencies, &mut scan.tools)?;
        }
        "package.json" => {
            collect_package_json_dependencies(
                &entry.path,
                &mut scan.dependencies,
                &mut scan.tools,
            )?;
        }
        "build.rs" => {
            let content = read_text(&entry.path)?;
            if content.contains("prost_build") || content.contains("protoc") {
                scan.tools.insert("protoc".to_string());
            }
        }
        _ => {}
    }
    Ok(())
}

fn read_readme_summary(source_path: &Utf8Path) -> Result<Option<String>, RepoctlError> {
    let path = source_path.join("README.md");
    if !path.is_file() {
        return Ok(None);
    }
    let content = read_text(&path)?;
    let summary = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .map(|line| line.trim_start_matches('#').trim())
        .collect::<Vec<_>>()
        .join(" ");
    Ok((!summary.is_empty()).then_some(summary))
}

fn is_primary_manifest(name: &str) -> bool {
    matches!(
        name,
        "Cargo.toml"
            | "Cargo.lock"
            | "pyproject.toml"
            | "uv.lock"
            | "poetry.lock"
            | "bun.lockb"
            | "tsconfig.json"
            | "rust-toolchain.toml"
            | "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lock"
            | "Pulumi.yaml"
            | "operon.yaml"
            | "Makefile"
            | "project.yaml"
            | "project.yml"
    )
}

fn collect_cargo_dependencies(
    path: &Utf8Path,
    dependencies: &mut BTreeSet<String>,
    tools: &mut BTreeSet<String>,
) -> Result<(), RepoctlError> {
    let content = read_text(path)?;
    let Ok(document) = content.parse::<DocumentMut>() else {
        return Ok(());
    };
    for table_name in [
        "dependencies",
        "dev-dependencies",
        "build-dependencies",
        "workspace.dependencies",
    ] {
        collect_toml_dependency_table(&document, table_name, dependencies);
    }
    if content.contains("prost-build") {
        tools.insert("protoc".to_string());
    }
    Ok(())
}

fn collect_toml_dependency_table(
    document: &DocumentMut,
    table_name: &str,
    dependencies: &mut BTreeSet<String>,
) {
    let Some(table) =
        dotted_toml_item(document.as_item(), table_name).and_then(Item::as_table_like)
    else {
        return;
    };
    for (name, item) in table.iter() {
        if name.starts_with("@cellis") || name == "operon" || item.to_string().contains("chromatin")
        {
            dependencies.insert(name.to_string());
        }
    }
}

fn dotted_toml_item<'a>(item: &'a Item, path: &str) -> Option<&'a Item> {
    let mut current = item;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

fn collect_package_json_dependencies(
    path: &Utf8Path,
    dependencies: &mut BTreeSet<String>,
    tools: &mut BTreeSet<String>,
) -> Result<(), RepoctlError> {
    let content = read_text(path)?;
    let Ok(value) = serde_json::from_str::<Value>(&content) else {
        return Ok(());
    };
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(object) = value.get(section).and_then(Value::as_object) {
            for key in object.keys() {
                dependencies.insert(key.clone());
            }
        }
    }
    if let Some(scripts) = value.get("scripts").and_then(Value::as_object) {
        for command in scripts.values().filter_map(Value::as_str) {
            if command.contains("pulumi") {
                tools.insert("pulumi".to_string());
            }
            if command.contains("buf") {
                tools.insert("buf".to_string());
            }
        }
    }
    tools.insert("node".to_string());
    Ok(())
}
