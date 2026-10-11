//! Nearest lockfile ownership shared by workspace synthesis and migration refresh plans.

use camino::{Utf8Path, Utf8PathBuf};
use repoctl_core::{Diagnostic, RepoctlError, SourceInventory};
use serde::Deserialize;

use super::source_walk::read_text;

#[derive(Debug)]
pub(super) struct NodeInstall {
    pub toolchain: &'static str,
    pub root: String,
    pub lockfile: Option<String>,
    pub cache_dir: &'static str,
}

pub(super) fn nearest_lockfile(
    inventory: &SourceInventory,
    root: &str,
    names: &[&str],
) -> Option<String> {
    Utf8Path::new(root).ancestors().find_map(|directory| {
        names.iter().find_map(|name| {
            let path = if matches!(directory.as_str(), "" | ".") {
                (*name).to_string()
            } else {
                format!("{directory}/{name}")
            };
            inventory.manifests.contains(&path).then_some(path)
        })
    })
}

pub(super) fn node_install(
    source: &Utf8Path,
    inventory: &SourceInventory,
    root: &str,
) -> Result<NodeInstall, RepoctlError> {
    let lockfile = nearest_lockfile(
        inventory,
        root,
        &[
            "pnpm-lock.yaml",
            "yarn.lock",
            "bun.lock",
            "bun.lockb",
            "package-lock.json",
        ],
    );
    let manager = match lockfile
        .as_deref()
        .and_then(|path| Utf8Path::new(path).file_name())
    {
        Some("pnpm-lock.yaml") => "pnpm",
        Some("yarn.lock") => "yarn",
        Some("bun.lock" | "bun.lockb") => "bun",
        Some("package-lock.json") => "npm",
        _ => declared_manager(source, inventory, root)?.unwrap_or("npm"),
    };
    let cache_dir = match manager {
        "pnpm" => "target/pnpm",
        "yarn" => "target/yarn",
        "bun" => "target/bun",
        _ => "target/npm",
    };
    let owner = lockfile
        .as_deref()
        .and_then(|path| Utf8Path::new(path).parent())
        .map_or_else(
            || root.to_string(),
            |path| {
                if path.as_str().is_empty() {
                    ".".to_string()
                } else {
                    path.to_string()
                }
            },
        );
    Ok(NodeInstall {
        toolchain: manager,
        root: owner,
        lockfile,
        cache_dir,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManagerManifest {
    package_manager: Option<String>,
}

fn declared_manager(
    source: &Utf8Path,
    inventory: &SourceInventory,
    root: &str,
) -> Result<Option<&'static str>, RepoctlError> {
    for directory in Utf8Path::new(root).ancestors() {
        let manifest = if matches!(directory.as_str(), "" | ".") {
            Utf8PathBuf::from("package.json")
        } else {
            directory.join("package.json")
        };
        if !inventory
            .manifests
            .iter()
            .any(|path| Utf8Path::new(path) == manifest)
        {
            continue;
        }
        let path = source.join(&manifest);
        let parsed: ManagerManifest =
            serde_json::from_str(&read_text(&path)?).map_err(|error| {
                RepoctlError::diagnostic(
                    Diagnostic::error("adoption.node.package_manager", error.to_string())
                        .with_path(path.as_str()),
                )
            })?;
        let Some(value) = parsed.package_manager else {
            continue;
        };
        return parse_manager(&value, &path).map(Some);
    }
    Ok(None)
}

fn parse_manager(value: &str, path: &Utf8Path) -> Result<&'static str, RepoctlError> {
    let manager = if value.len() <= 256 && value.bytes().all(|byte| byte.is_ascii_graphic()) {
        value.split_once('@').and_then(|(manager, version)| {
            if version.is_empty() {
                return None;
            }
            match manager {
                "npm" => Some("npm"),
                "pnpm" => Some("pnpm"),
                "yarn" => Some("yarn"),
                "bun" => Some("bun"),
                _ => None,
            }
        })
    } else {
        None
    };
    manager.ok_or_else(|| {
        RepoctlError::diagnostic(
            Diagnostic::error(
                "adoption.node.package_manager",
                "packageManager must select npm, pnpm, yarn or bun with a nonempty version \
                 (maximum 256 ASCII bytes)",
            )
            .with_path(path.as_str()),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs};

    use camino::Utf8Path;
    use repoctl_core::SourceInventory;

    use super::node_install;

    #[test]
    fn test_should_prefer_existing_lockfile_to_package_manager_declaration()
    -> Result<(), Box<dyn Error>> {
        let temp = tempfile::tempdir()?;
        let source = Utf8Path::from_path(temp.path()).ok_or("non-UTF8 test path")?;
        fs::write(
            source.join("package.json"),
            r#"{"packageManager":"yarn@4.0.0"}"#,
        )?;
        let inventory = SourceInventory {
            manifests: vec!["package.json".to_string(), "pnpm-lock.yaml".to_string()],
            ..SourceInventory::default()
        };
        let install = node_install(source, &inventory, ".")?;
        assert_eq!(install.toolchain, "pnpm");
        assert_eq!(install.lockfile.as_deref(), Some("pnpm-lock.yaml"));
        Ok(())
    }
}
