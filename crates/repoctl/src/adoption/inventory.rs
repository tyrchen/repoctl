//! Local source inventory and dependency references.

use std::{collections::BTreeSet, fs};

use camino::{Utf8Path, Utf8PathBuf};
use repoctl_core::{Diagnostic, RepoctlError, SourceInventory};
use serde_json::Value;
use toml_edit::{DocumentMut, Item};

use super::ADOPTION_EXCLUDED_DIRS;

pub(super) fn source_repositories(
    source_root: &Utf8Path,
) -> Result<Vec<Utf8PathBuf>, RepoctlError> {
    if source_root.join("Cargo.toml").is_file()
        || source_root.join("package.json").is_file()
        || source_root.join("project.yaml").is_file()
        || source_root.join("Pulumi.yaml").is_file()
    {
        return Ok(vec![source_root.to_path_buf()]);
    }
    let mut repos = Vec::new();
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
        if path.is_dir()
            && (path.join("Cargo.toml").is_file()
                || path.join("package.json").is_file()
                || path.join("project.yaml").is_file()
                || path.join("Pulumi.yaml").is_file()
                || path.join("README.md").is_file())
        {
            repos.push(path);
        }
    }
    repos.sort();
    Ok(repos)
}

pub(super) fn source_name(path: &Utf8Path) -> Result<String, RepoctlError> {
    path.file_name().map(ToString::to_string).ok_or_else(|| {
        RepoctlError::diagnostic(Diagnostic::error(
            "adoption.source.name",
            "source path must have a directory name",
        ))
    })
}

pub(super) fn inventory_source(
    source_path: &Utf8Path,
    name: &str,
) -> Result<SourceInventory, RepoctlError> {
    let mut manifests = Vec::new();
    let mut top_level_dirs = Vec::new();
    let mut dependencies = BTreeSet::new();
    let mut generated = Vec::new();
    let mut tools = BTreeSet::new();
    let has_vcs = source_path.join(".git").exists();
    let readme_summary = read_readme_summary(source_path)?;

    for entry in fs::read_dir(source_path)
        .map_err(|source| RepoctlError::io(source_path.to_path_buf(), source))?
    {
        let entry = entry.map_err(|source| RepoctlError::io(source_path.to_path_buf(), source))?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|path| {
            RepoctlError::diagnostic(Diagnostic::error(
                "path.non_utf8",
                format!("path is not valid UTF-8: {}", path.display()),
            ))
        })?;
        let Some(file_name) = path.file_name() else {
            continue;
        };
        if path.is_dir() {
            top_level_dirs.push(file_name.to_string());
        }
    }

    collect_inventory_files(
        source_path,
        source_path,
        &mut manifests,
        &mut dependencies,
        &mut generated,
        &mut tools,
    )?;
    top_level_dirs.sort();
    manifests.sort();
    generated.sort();
    Ok(SourceInventory {
        name: name.to_string(),
        has_vcs,
        readme_summary,
        manifests,
        top_level_dirs,
        dependency_references: dependencies.into_iter().collect(),
        generated_artifacts: generated,
        required_tools: tools.into_iter().collect(),
    })
}

fn collect_inventory_files(
    root: &Utf8Path,
    current: &Utf8Path,
    manifests: &mut Vec<String>,
    dependencies: &mut BTreeSet<String>,
    generated: &mut Vec<String>,
    tools: &mut BTreeSet<String>,
) -> Result<(), RepoctlError> {
    for entry in
        fs::read_dir(current).map_err(|source| RepoctlError::io(current.to_path_buf(), source))?
    {
        let entry = entry.map_err(|source| RepoctlError::io(current.to_path_buf(), source))?;
        let path = Utf8PathBuf::from_path_buf(entry.path()).map_err(|path| {
            RepoctlError::diagnostic(Diagnostic::error(
                "path.non_utf8",
                format!("path is not valid UTF-8: {}", path.display()),
            ))
        })?;
        if path.is_symlink() {
            return Err(RepoctlError::diagnostic(Diagnostic::error(
                "adoption.copy.symlink",
                format!("adoption source contains a symlink: {path}"),
            )));
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        let rel = path
            .strip_prefix(root)
            .map_or_else(|_| name.to_string(), |value| value.as_str().to_string());
        if path.is_dir() {
            if ADOPTION_EXCLUDED_DIRS.contains(&name) {
                generated.push(rel);
                continue;
            }
            collect_inventory_files(root, &path, manifests, dependencies, generated, tools)?;
            continue;
        }
        if is_primary_manifest(name)
            || rel.ends_with("/package.json")
            || rel.ends_with("/Cargo.toml")
        {
            manifests.push(rel.clone());
        }
        if name == "Cargo.toml" {
            collect_cargo_dependencies(&path, dependencies, tools)?;
        } else if name == "package.json" {
            collect_package_json_dependencies(&path, dependencies, tools)?;
        } else if name == "build.rs" {
            let content = fs::read_to_string(&path)
                .map_err(|source| RepoctlError::io(path.clone(), source))?;
            if content.contains("prost_build") || content.contains("protoc") {
                tools.insert("protoc".to_string());
            }
        }
    }
    Ok(())
}

fn read_readme_summary(source_path: &Utf8Path) -> Result<Option<String>, RepoctlError> {
    let path = source_path.join("README.md");
    if !path.is_file() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path).map_err(|source| RepoctlError::io(path, source))?;
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
    let content =
        fs::read_to_string(path).map_err(|source| RepoctlError::io(path.to_path_buf(), source))?;
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
    let content =
        fs::read_to_string(path).map_err(|source| RepoctlError::io(path.to_path_buf(), source))?;
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
