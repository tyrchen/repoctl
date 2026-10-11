//! Invalidate stale Node locks and schedule explicit package-manager regeneration.

// Refresh planning synchronously reads only a bounded lockfile prefix.
#![allow(clippy::disallowed_types)]

use std::{collections::BTreeMap, fs::File, io::Read as _};

use camino::Utf8Path;
use repoctl_core::{
    AdoptedSource, AdoptionFileOperation, DependencyRewrite, Diagnostic, LockfileRefresh,
    ProcessCommand, ProjectManifestSynthesis, ProjectRelativePath, RepoRelativePath, RepoctlError,
};

use super::{node::node_install, process_command};

pub(super) fn plan_refreshes(
    destination: &Utf8Path,
    sources: &[AdoptedSource],
    rewrites: &[DependencyRewrite],
    operations: &mut Vec<AdoptionFileOperation>,
    syntheses: &mut [ProjectManifestSynthesis],
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
            if let Some(refresh) = refresh_for_manifest(
                destination,
                source,
                manifest,
                rewrites,
                operations,
                syntheses,
            )? {
                refreshes.insert(refresh.lockfile.clone(), refresh);
            }
        }
    }
    for refresh in refreshes.values() {
        diagnostics.push(
            Diagnostic::warning(
                "adoption.lockfile.refresh_required",
                format!(
                    "regenerate `{}` with the planned refresh after apply and commit it; the \
                     original owning lockfile is omitted from copy operations",
                    refresh.lockfile
                ),
            )
            .with_path(refresh.lockfile.as_str()),
        );
    }
    Ok(refreshes.into_values().collect())
}

fn refresh_for_manifest(
    destination: &Utf8Path,
    source: &AdoptedSource,
    manifest: &str,
    rewrites: &[DependencyRewrite],
    operations: &mut Vec<AdoptionFileOperation>,
    syntheses: &mut [ProjectManifestSynthesis],
) -> Result<Option<LockfileRefresh>, RepoctlError> {
    let file = source
        .destination_path
        .join_project(&ProjectRelativePath::new(manifest).map_err(RepoctlError::diagnostic)?)
        .map_err(RepoctlError::diagnostic)?;
    if !rewrites.iter().any(|rewrite| rewrite.file == file) {
        return Ok(None);
    }
    let root = manifest.strip_suffix("/package.json").unwrap_or(".");
    let install = node_install(&source.inventory, root);
    let Some(lockfile) = install.lockfile else {
        return Ok(None);
    };
    let stale_path = source
        .destination_path
        .join_project(&ProjectRelativePath::new(&lockfile).map_err(RepoctlError::diagnostic)?)
        .map_err(RepoctlError::diagnostic)?;
    let cwd = source
        .destination_path
        .join_project(&ProjectRelativePath::new(&install.root).map_err(RepoctlError::diagnostic)?)
        .map_err(RepoctlError::diagnostic)?;
    let command = refresh_command(
        destination,
        &cwd,
        install.toolchain,
        &source.source_path.join(&lockfile),
    )?;
    let replacement = if let Some(prefix) = lockfile.strip_suffix("bun.lockb") {
        format!("{prefix}bun.lock")
    } else {
        lockfile.clone()
    };
    let path = source
        .destination_path
        .join_project(&ProjectRelativePath::new(&replacement).map_err(RepoctlError::diagnostic)?)
        .map_err(RepoctlError::diagnostic)?;
    operations.retain(|operation| {
        operation.destination_path != stale_path && operation.destination_path != path
    });
    if replacement != lockfile {
        retarget_lockfile(&source.name, &lockfile, &replacement, syntheses, operations);
    }
    Ok(Some(LockfileRefresh::new(path, command)))
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
                let mut text = Vec::new();
                File::open(source_lock)
                    .map_err(|error| RepoctlError::io(source_lock.to_path_buf(), error))?
                    .take(8192)
                    .read_to_end(&mut text)
                    .map_err(|error| RepoctlError::io(source_lock.to_path_buf(), error))?;
                if text
                    .windows(b"__metadata:".len())
                    .any(|part| part == b"__metadata:")
                {
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
    if source_lock.file_name() == Some("bun.lockb") {
        command.args.push("--save-text-lockfile".to_string());
    }
    Ok(command)
}

fn retarget_lockfile(
    source: &str,
    original: &str,
    replacement: &str,
    syntheses: &mut [ProjectManifestSynthesis],
    operations: &mut [AdoptionFileOperation],
) {
    for synthesis in syntheses
        .iter_mut()
        .filter(|synthesis| synthesis.source == source)
    {
        // These exact lines are emitted by our manifest synthesizer, not parsed user YAML.
        synthesis.content = synthesis.content.replace(
            &format!("    lockfile: {original}\n"),
            &format!("    lockfile: {replacement}\n"),
        );
        if let Some(operation) = operations
            .iter_mut()
            .find(|operation| operation.destination_path == synthesis.manifest_path)
        {
            operation.content = Some(synthesis.content.clone());
        }
    }
}
