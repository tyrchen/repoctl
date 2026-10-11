// Discovery ports are synchronous CLI operations, never async request handlers.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

//! Offline package dependency discovery without registry access or package-manager execution.

mod cargo;
mod packages;

use std::{
    collections::BTreeMap,
    fs,
    io::Read as _,
    path::{Component, Path, PathBuf},
};

use packages::{PackageIndex, PackageManifest, inspect_packages, package_index};
use repoctl_core::{
    Diagnostic, DiscoveredEdge, GraphBuildInput, RepoRelativePath, RepoctlError,
    WorkspaceInspectionInput, WorkspaceInspector, WorkspaceLanguage,
};
use serde_json::Value;
use toml_edit::{DocumentMut, Item};

use crate::adapters::classify_path_dependency;

const MAX_MANIFEST_BYTES: u64 = 262_144;

/// Cargo path-dependency inspection using only local TOML files.
#[derive(Clone, Debug, Default)]
pub struct LocalRustWorkspaceInspector;

impl WorkspaceInspector for LocalRustWorkspaceInspector {
    fn language(&self) -> WorkspaceLanguage {
        WorkspaceLanguage::Rust
    }

    fn inspect(
        &self,
        input: &WorkspaceInspectionInput<'_>,
    ) -> Result<Vec<DiscoveredEdge>, RepoctlError> {
        let path = manifest_path(input)?;
        let Some(text) = read_manifest(&path, input.root.absolute.as_std_path())? else {
            return Ok(Vec::new());
        };
        let doc = parse_toml(&text, &path)?;
        cargo::inspect(input, &path, &doc)
    }
}

/// Node package inspection for TypeScript workspaces.
#[derive(Clone, Debug, Default)]
pub struct NodeWorkspaceInspector;

impl WorkspaceInspector for NodeWorkspaceInspector {
    fn language(&self) -> WorkspaceLanguage {
        WorkspaceLanguage::TypeScript
    }

    fn inspect(
        &self,
        input: &WorkspaceInspectionInput<'_>,
    ) -> Result<Vec<DiscoveredEdge>, RepoctlError> {
        let index = package_index(
            input.root,
            input.projects,
            &self.language(),
            &mut read_manifest,
        )?;
        inspect_indexed(input, &index)
    }

    fn inspect_all(&self, input: &GraphBuildInput) -> Result<Vec<DiscoveredEdge>, RepoctlError> {
        inspect_packages(input, &self.language(), &mut read_manifest)
    }
}

fn node_edges(
    input: &WorkspaceInspectionInput<'_>,
    path: &Path,
    doc: &Value,
    names: &BTreeMap<String, RepoRelativePath>,
) -> Result<Vec<DiscoveredEdge>, RepoctlError> {
    let mut edges = Vec::new();
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(deps) = doc.get(section).and_then(Value::as_object) {
            for (name, version) in deps {
                let version = version
                    .as_str()
                    .ok_or_else(|| manifest_error(path, "dependency versions must be strings"))?;
                if let Some(local) = version
                    .strip_prefix("file:")
                    .or_else(|| version.strip_prefix("link:"))
                {
                    append_path_edge(input, path, local, name, &mut edges)?;
                } else if let Some(target) = names.get(name) {
                    append_target_edge(input, target, path, name, &mut edges);
                }
            }
        }
    }
    Ok(edges)
}

/// Python distribution and local-source inspection for PEP 621, uv and Poetry.
#[derive(Clone, Debug, Default)]
pub struct PythonWorkspaceInspector;

impl WorkspaceInspector for PythonWorkspaceInspector {
    fn language(&self) -> WorkspaceLanguage {
        WorkspaceLanguage::Python
    }

    fn inspect(
        &self,
        input: &WorkspaceInspectionInput<'_>,
    ) -> Result<Vec<DiscoveredEdge>, RepoctlError> {
        let index = package_index(
            input.root,
            input.projects,
            &self.language(),
            &mut read_manifest,
        )?;
        inspect_indexed(input, &index)
    }

    fn inspect_all(&self, input: &GraphBuildInput) -> Result<Vec<DiscoveredEdge>, RepoctlError> {
        inspect_packages(input, &self.language(), &mut read_manifest)
    }
}

fn python_edges(
    input: &WorkspaceInspectionInput<'_>,
    path: &Path,
    doc: &DocumentMut,
    names: &BTreeMap<String, RepoRelativePath>,
) -> Result<Vec<DiscoveredEdge>, RepoctlError> {
    let mut edges = Vec::new();
    if let Some(deps) = doc.get("project").and_then(|p| p.get("dependencies")) {
        python_requirements(input, deps, names, path, &mut edges)?;
    }
    if let Some(groups) = doc
        .get("project")
        .and_then(|p| p.get("optional-dependencies"))
        .and_then(Item::as_table_like)
    {
        for (_, deps) in groups.iter() {
            python_requirements(input, deps, names, path, &mut edges)?;
        }
    }
    if let Some(groups) = doc.get("dependency-groups").and_then(Item::as_table_like) {
        for (_, deps) in groups.iter() {
            python_requirements(input, deps, names, path, &mut edges)?;
        }
    }
    for parts in [
        ["tool", "uv", "sources"],
        ["tool", "poetry", "dependencies"],
    ] {
        if let Some(deps) = toml_path(doc.as_item(), &parts).and_then(Item::as_table_like) {
            for (name, source) in deps.iter() {
                if let Some(local) = source.get("path").and_then(Item::as_str) {
                    append_path_edge(input, path, local, name, &mut edges)?;
                } else if let Some(target) = names.get(&normalize_python_name(name)) {
                    append_target_edge(input, target, path, name, &mut edges);
                }
            }
        }
    }
    Ok(edges)
}

fn python_requirements(
    input: &WorkspaceInspectionInput<'_>,
    deps: &Item,
    names: &BTreeMap<String, RepoRelativePath>,
    manifest: &Path,
    edges: &mut Vec<DiscoveredEdge>,
) -> Result<(), RepoctlError> {
    let array = deps
        .as_array()
        .ok_or_else(|| manifest_error(manifest, "Python requirements must be arrays"))?;
    for requirement in array.iter().filter_map(toml_edit::Value::as_str) {
        let name = requirement
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
            .collect::<String>();
        if let Some(target) = names.get(&normalize_python_name(&name)) {
            append_target_edge(input, target, manifest, requirement, edges);
        }
    }
    Ok(())
}

fn inspect_indexed(
    input: &WorkspaceInspectionInput<'_>,
    index: &PackageIndex,
) -> Result<Vec<DiscoveredEdge>, RepoctlError> {
    let path = manifest_path(input)?;
    match index.documents.get(&path) {
        Some(PackageManifest::Node(doc)) => node_edges(input, &path, doc, &index.names),
        Some(PackageManifest::Python(doc)) => python_edges(input, &path, doc, &index.names),
        None => Ok(Vec::new()),
    }
}

fn normalize_python_name(name: &str) -> String {
    name.to_ascii_lowercase()
        .split(['-', '_', '.'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn append_target_edge(
    input: &WorkspaceInspectionInput<'_>,
    target: &RepoRelativePath,
    manifest: &Path,
    name: &str,
    edges: &mut Vec<DiscoveredEdge>,
) {
    if let Some(mut edge) = classify_path_dependency(input, target) {
        edge.evidence = Some(format!(
            "{}: {name}",
            manifest
                .strip_prefix(input.root.absolute.as_std_path())
                .unwrap_or(manifest)
                .display()
        ));
        edges.push(edge);
    }
}

fn append_path_edge(
    input: &WorkspaceInspectionInput<'_>,
    manifest: &Path,
    local: &str,
    name: &str,
    edges: &mut Vec<DiscoveredEdge>,
) -> Result<(), RepoctlError> {
    if local.len() > 4096 || local.contains('\0') {
        return Err(manifest_error(manifest, "invalid dependency path"));
    }
    let parent = manifest
        .parent()
        .ok_or_else(|| manifest_error(manifest, "manifest has no parent"))?;
    let mut target = PathBuf::new();
    for part in parent.join(local).components() {
        match part {
            Component::ParentDir => {
                target.pop();
            }
            Component::CurDir => {}
            _ => target.push(part.as_os_str()),
        }
    }
    if target.exists() {
        target = fs::canonicalize(&target)
            .map_err(|source| RepoctlError::io(target.display().to_string(), source))?;
    }
    let relative = target
        .strip_prefix(input.root.absolute.as_std_path())
        .map_err(|_| manifest_error(manifest, "local dependency escapes repository"))?;
    let relative = RepoRelativePath::new(relative.to_string_lossy().replace('\\', "/"))
        .map_err(RepoctlError::diagnostic)?;
    append_target_edge(input, &relative, manifest, name, edges);
    Ok(())
}

fn manifest_path(input: &WorkspaceInspectionInput<'_>) -> Result<PathBuf, RepoctlError> {
    let rel = input
        .project
        .path
        .join_project(&input.workspace.manifest)
        .map_err(RepoctlError::diagnostic)?;
    Ok(input.root.join(&rel).into_std_path_buf())
}

fn read_manifest(path: &Path, root: &Path) -> Result<Option<String>, RepoctlError> {
    if !path.exists() {
        return Ok(None);
    }
    let canonical = fs::canonicalize(path)
        .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
    if !canonical.starts_with(root) {
        return Err(manifest_error(path, "manifest escapes repository"));
    }
    let metadata = fs::metadata(&canonical)
        .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
    if !metadata.is_file() {
        return Err(manifest_error(path, "manifest must be a regular file"));
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(manifest_error(path, "manifest exceeds 256 KiB"));
    }
    let file = fs::File::open(&canonical)
        .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| RepoctlError::io(path.display().to_string(), source))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(manifest_error(path, "manifest exceeds 256 KiB"));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|error| manifest_error(path, &error.to_string()))
}

fn parse_toml(text: &str, path: &Path) -> Result<DocumentMut, RepoctlError> {
    text.parse()
        .map_err(|error: toml_edit::TomlError| manifest_error(path, &error.to_string()))
}

fn manifest_error(path: &Path, message: &str) -> RepoctlError {
    RepoctlError::diagnostic(
        Diagnostic::error("graph.local_manifest", message).with_path(path.display().to_string()),
    )
}

fn toml_path<'a>(item: &'a Item, parts: &[&str]) -> Option<&'a Item> {
    parts
        .iter()
        .try_fold(item, |current, part| current.get(part))
}

fn cargo_tables(
    doc: &DocumentMut,
    callback: &mut impl FnMut(&dyn toml_edit::TableLike) -> Result<(), RepoctlError>,
) -> Result<(), RepoctlError> {
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = doc.get(section).and_then(Item::as_table_like) {
            callback(table)?;
        }
        if let Some(targets) = doc.get("target").and_then(Item::as_table_like) {
            for (_, target) in targets.iter() {
                if let Some(table) = target.get(section).and_then(Item::as_table_like) {
                    callback(table)?;
                }
            }
        }
    }
    Ok(())
}

fn inherited_cargo_dependency(
    path: &Path,
    name: &str,
    input: &WorkspaceInspectionInput<'_>,
) -> Result<Option<(PathBuf, String)>, RepoctlError> {
    if let Some(text) = read_manifest(path, input.root.absolute.as_std_path())? {
        let doc = parse_toml(&text, path)?;
        if let Some(workspace) =
            toml_path(doc.as_item(), &["package", "workspace"]).and_then(Item::as_str)
        {
            let parent = path
                .parent()
                .ok_or_else(|| manifest_error(path, "manifest has no parent"))?;
            let manifest = parent.join(workspace).join("Cargo.toml");
            let text =
                read_manifest(&manifest, input.root.absolute.as_std_path())?.ok_or_else(|| {
                    manifest_error(path, "explicit Cargo workspace manifest is missing")
                })?;
            let doc = parse_toml(&text, &manifest)?;
            return Ok(
                toml_path(doc.as_item(), &["workspace", "dependencies", name, "path"])
                    .and_then(Item::as_str)
                    .map(|local| (manifest, local.to_string())),
            );
        }
    }
    for ancestor in path
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .filter(|p| p.starts_with(input.root.absolute.as_std_path()))
    {
        let manifest = ancestor.join("Cargo.toml");
        if let Some(text) = read_manifest(&manifest, input.root.absolute.as_std_path())? {
            let doc = parse_toml(&text, &manifest)?;
            if doc.get("workspace").is_some() {
                return Ok(
                    toml_path(doc.as_item(), &["workspace", "dependencies", name, "path"])
                        .and_then(Item::as_str)
                        .map(|local| (manifest, local.to_string())),
                );
            }
        }
    }
    Ok(None)
}
