//! Invalidate stale Node locks and schedule explicit package-manager regeneration.

use std::{collections::BTreeMap, fs};

use camino::Utf8Path;
use repoctl_core::{
    AdoptedSource, AdoptionFileOperation, DependencyRewrite, Diagnostic, LockfileRefresh,
    ProcessCommand, ProjectRelativePath, RepoRelativePath, RepoctlError,
};

use super::{node::node_install, process_command};

pub(super) fn plan_refreshes(
    destination: &Utf8Path,
    sources: &[AdoptedSource],
    rewrites: &[DependencyRewrite],
    operations: &mut Vec<AdoptionFileOperation>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Vec<LockfileRefresh>, RepoctlError> {
    let mut refreshes = BTreeMap::new();
    for source in sources.iter().filter(|source| !source.skipped) {
        for manifest in source
            .inventory
            .manifests
            .iter()
            .filter(|path| path.ends_with("package.json"))
        {
            let file = source
                .destination_path
                .join_project(
                    &ProjectRelativePath::new(manifest).map_err(RepoctlError::diagnostic)?,
                )
                .map_err(RepoctlError::diagnostic)?;
            if !rewrites.iter().any(|rewrite| rewrite.file == file) {
                continue;
            }
            let root = manifest.strip_suffix("/package.json").unwrap_or(".");
            let install = node_install(&source.inventory, root);
            let Some(lockfile) = install.lockfile else {
                continue;
            };
            let path = source
                .destination_path
                .join_project(
                    &ProjectRelativePath::new(&lockfile).map_err(RepoctlError::diagnostic)?,
                )
                .map_err(RepoctlError::diagnostic)?;
            let cwd = source
                .destination_path
                .join_project(
                    &ProjectRelativePath::new(&install.root).map_err(RepoctlError::diagnostic)?,
                )
                .map_err(RepoctlError::diagnostic)?;
            let command = refresh_command(
                destination,
                &cwd,
                install.toolchain,
                &source.source_path.join(&lockfile),
            )?;
            operations.retain(|operation| operation.destination_path != path);
            refreshes.insert(path.clone(), LockfileRefresh::new(path, command));
        }
    }
    for refresh in refreshes.values() {
        diagnostics.push(
            Diagnostic::warning(
                "adoption.lockfile.refresh_required",
                format!(
                    "stale `{}` will not be copied; run the planned refresh after apply and \
                     commit the regenerated lockfile",
                    refresh.lockfile
                ),
            )
            .with_path(refresh.lockfile.as_str()),
        );
    }
    Ok(refreshes.into_values().collect())
}

fn refresh_command(
    destination: &Utf8Path,
    cwd: &RepoRelativePath,
    toolchain: &str,
    source_lock: &Utf8Path,
) -> Result<ProcessCommand, RepoctlError> {
    let mut command = process_command(destination, cwd, toolchain, ["install"]);
    command.args.extend(
        match toolchain {
            "npm" => vec!["--package-lock-only", "--ignore-scripts"],
            "yarn" => {
                let text = fs::read_to_string(source_lock)
                    .map_err(|error| RepoctlError::io(source_lock.to_path_buf(), error))?;
                if text.contains("__metadata:") {
                    vec!["--mode=update-lockfile"]
                } else {
                    vec!["--ignore-scripts"]
                }
            }
            _ => vec!["--lockfile-only", "--ignore-scripts"],
        }
        .into_iter()
        .map(ToString::to_string),
    );
    Ok(command)
}
