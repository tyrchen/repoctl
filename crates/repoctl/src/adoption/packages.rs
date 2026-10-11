//! Unambiguous Cargo and Node package indexes for generic adoption.

use std::{collections::BTreeMap, fs};

use camino::{Utf8Path, Utf8PathBuf};
use repoctl_core::{
    AdoptedSource, Diagnostic, ProjectRelativePath, RepoRelativePath, RepoctlError,
};
use serde_json::Value;
use toml_edit::{DocumentMut, Item};

use super::ADOPTION_EXCLUDED_DIRS;

#[derive(Clone, Debug, Default)]
pub(super) struct PackageIndex {
    pub(super) rust: BTreeMap<String, RepoRelativePath>,
    pub(super) node: BTreeMap<String, RepoRelativePath>,
}

pub(super) fn build_adopted_package_index(
    sources: &[AdoptedSource],
) -> Result<PackageIndex, RepoctlError> {
    let mut index = PackageIndex::default();
    for source in sources.iter().filter(|source| !source.skipped) {
        collect_package_index(
            &source.source_path,
            &source.source_path,
            &source.destination_path,
            &mut index,
        )?;
    }
    Ok(index)
}

fn collect_package_index(
    root: &Utf8Path,
    current: &Utf8Path,
    destination: &RepoRelativePath,
    index: &mut PackageIndex,
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
        if path.is_dir() {
            if ADOPTION_EXCLUDED_DIRS.contains(&name) {
                continue;
            }
            collect_package_index(root, &path, destination, index)?;
            continue;
        }
        index_package(root, &path, destination, index)?;
    }
    Ok(())
}

fn index_package(
    root: &Utf8Path,
    path: &Utf8Path,
    destination: &RepoRelativePath,
    index: &mut PackageIndex,
) -> Result<(), RepoctlError> {
    let name = path.file_name().unwrap_or_default();
    let relative = path.strip_prefix(root).map_err(|_| {
        RepoctlError::diagnostic(Diagnostic::error(
            "adoption.package.path",
            "package path escaped source root",
        ))
    })?;
    let package_root = relative
        .parent()
        .filter(|path| !path.as_str().is_empty())
        .unwrap_or_else(|| Utf8Path::new("."));
    let package_root = ProjectRelativePath::new(package_root.as_str().to_string())
        .map_err(RepoctlError::diagnostic)?;
    let dest_package_root = destination
        .join_project(&package_root)
        .map_err(RepoctlError::diagnostic)?;
    if name == "Cargo.toml" {
        if let Some(package) = cargo_package_name(path)?
            && index
                .rust
                .insert(package.clone(), dest_package_root)
                .is_some()
        {
            return Err(RepoctlError::diagnostic(Diagnostic::error(
                "adoption.package.ambiguous",
                format!("duplicate Cargo package `{package}`"),
            )));
        }
    } else if name == "package.json"
        && let Some(package) = node_package_name(path)?
        && index
            .node
            .insert(package.clone(), dest_package_root)
            .is_some()
    {
        return Err(RepoctlError::diagnostic(Diagnostic::error(
            "adoption.package.ambiguous",
            format!("duplicate Node package `{package}`"),
        )));
    }
    Ok(())
}

fn cargo_package_name(path: &Utf8Path) -> Result<Option<String>, RepoctlError> {
    let content =
        fs::read_to_string(path).map_err(|source| RepoctlError::io(path.to_path_buf(), source))?;
    let document = content.parse::<DocumentMut>().map_err(|error| {
        RepoctlError::Environment(format!("invalid Cargo manifest `{path}`: {error}"))
    })?;
    Ok(document
        .get("package")
        .and_then(|item| item.get("name"))
        .and_then(Item::as_str)
        .map(ToString::to_string))
}

fn node_package_name(path: &Utf8Path) -> Result<Option<String>, RepoctlError> {
    let content =
        fs::read_to_string(path).map_err(|source| RepoctlError::io(path.to_path_buf(), source))?;
    let value = serde_json::from_str::<Value>(&content).map_err(|error| {
        RepoctlError::Environment(format!("invalid Node manifest `{path}`: {error}"))
    })?;
    Ok(value
        .get("name")
        .and_then(Value::as_str)
        .map(ToString::to_string))
}
