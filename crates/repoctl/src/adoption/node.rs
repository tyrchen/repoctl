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

pub(super) fn node_install(inventory: &SourceInventory, root: &str) -> NodeInstall {
    for directory in Utf8Path::new(root).ancestors() {
        let directory = if matches!(directory.as_str(), "" | ".") {
            "."
        } else {
            directory.as_str()
        };
        for (toolchain, name, cache_dir) in [
            ("pnpm", "pnpm-lock.yaml", "target/pnpm"),
            ("yarn", "yarn.lock", "target/yarn"),
            ("bun", "bun.lock", "target/bun"),
            ("npm", "package-lock.json", "target/npm"),
        ] {
            let path = if directory == "." {
                name.to_string()
            } else {
                format!("{directory}/{name}")
            };
            if inventory.manifests.contains(&path) {
                return NodeInstall {
                    toolchain,
                    root: directory.to_string(),
                    lockfile: Some(path),
                    cache_dir,
                };
            }
        }
    }
    NodeInstall {
        toolchain: "npm",
        root: root.to_string(),
        lockfile: None,
        cache_dir: "target/npm",
    }
}
