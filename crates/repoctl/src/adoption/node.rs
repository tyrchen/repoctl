//! Nearest lockfile ownership shared by workspace synthesis and migration refresh plans.

use camino::Utf8Path;
use repoctl_core::SourceInventory;

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

pub(super) fn node_install(inventory: &SourceInventory, root: &str) -> NodeInstall {
    let lockfile = nearest_lockfile(
        inventory,
        root,
        &[
            "pnpm-lock.yaml",
            "yarn.lock",
            "bun.lock",
            "package-lock.json",
        ],
    );
    let (toolchain, cache_dir) = match lockfile
        .as_deref()
        .and_then(|path| Utf8Path::new(path).file_name())
    {
        Some("pnpm-lock.yaml") => ("pnpm", "target/pnpm"),
        Some("yarn.lock") => ("yarn", "target/yarn"),
        Some("bun.lock") => ("bun", "target/bun"),
        _ => ("npm", "target/npm"),
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
    NodeInstall {
        toolchain,
        root: owner,
        lockfile,
        cache_dir,
    }
}
