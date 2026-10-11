//! Bounded saved-plan parsing shared by adoption apply and verify.

// Saved plans are synchronous CLI inputs, outside an async runtime.
#![allow(clippy::disallowed_types)]

use std::{
    fs::{self, File},
    io::{self, Read as _, Write},
    path::Path,
};

use repoctl_core::{AdoptionPlan, Diagnostic, ProcessCommand, RepoctlError};

use super::source_walk::{MAX_ENTRIES, MAX_FILES, MAX_SOURCES};

const MAX_PLAN_BYTES: usize = 67_108_864;
const MAX_SMALL_COLLECTION: usize = 256;

pub(crate) fn read_adoption_plan(path: &Path) -> Result<AdoptionPlan, RepoctlError> {
    let metadata =
        fs::metadata(path).map_err(|error| RepoctlError::io(path.display().to_string(), error))?;
    if !metadata.is_file() || metadata.len() > MAX_PLAN_BYTES as u64 {
        return Err(plan_limit("bytes", MAX_PLAN_BYTES));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|error| RepoctlError::io(path.display().to_string(), error))?
        .take(MAX_PLAN_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| RepoctlError::io(path.display().to_string(), error))?;
    if bytes.len() > MAX_PLAN_BYTES {
        return Err(plan_limit("bytes", MAX_PLAN_BYTES));
    }
    let plan: AdoptionPlan = serde_json::from_slice(&bytes).map_err(|error| {
        RepoctlError::diagnostic(
            Diagnostic::error(
                "adoption.plan.json",
                format!("failed to parse plan JSON: {error}"),
            )
            .with_path(path.display().to_string()),
        )
    })?;
    validate_plan_limits(&plan)?;
    Ok(plan)
}

pub(super) fn validate_plan_limits(plan: &AdoptionPlan) -> Result<(), RepoctlError> {
    counts(&[
        ("sources", plan.sources.len(), MAX_SOURCES),
        ("operations", plan.operations.len(), MAX_FILES),
        (
            "dependency_rewrites",
            plan.dependency_rewrites.len(),
            MAX_FILES,
        ),
        (
            "lockfile_refreshes",
            plan.lockfile_refreshes.len(),
            MAX_FILES,
        ),
        (
            "manifest_syntheses",
            plan.manifest_syntheses.len(),
            MAX_SOURCES,
        ),
        ("ci_operations", plan.ci_operations.len(), 1),
        (
            "prerequisites",
            plan.verification.prerequisites.len(),
            MAX_SMALL_COLLECTION,
        ),
        ("commands", plan.verification.commands.len(), MAX_FILES),
        ("diagnostics", plan.diagnostics.len(), MAX_FILES),
    ])?;
    for source in &plan.sources {
        let inventory = &source.inventory;
        counts(&[
            ("source.reasons", source.reasons.len(), MAX_SMALL_COLLECTION),
            ("inventory.manifests", inventory.manifests.len(), MAX_FILES),
            (
                "inventory.top_level_dirs",
                inventory.top_level_dirs.len(),
                MAX_ENTRIES,
            ),
            (
                "inventory.dependency_references",
                inventory.dependency_references.len(),
                MAX_FILES,
            ),
            (
                "inventory.generated_artifacts",
                inventory.generated_artifacts.len(),
                MAX_FILES,
            ),
            (
                "inventory.required_tools",
                inventory.required_tools.len(),
                MAX_SMALL_COLLECTION,
            ),
        ])?;
    }
    for rewrite in &plan.dependency_rewrites {
        counts(&[("rewrite.table", rewrite.table.len(), 64)])?;
    }
    for command in plan.verification.commands.iter().chain(
        plan.lockfile_refreshes
            .iter()
            .map(|refresh| &refresh.command),
    ) {
        validate_command_collections(command)?;
    }
    // Match the CLI's reviewed pretty JSON without allocating a second copy.
    let mut size = PlanSize { bytes: 1 };
    serde_json::to_writer_pretty(&mut size, plan)
        .map_err(|_| plan_limit("bytes", MAX_PLAN_BYTES))?;
    Ok(())
}

fn validate_command_collections(command: &ProcessCommand) -> Result<(), RepoctlError> {
    let cache = &command.execution.cache;
    counts(&[
        ("command.args", command.args.len(), MAX_SMALL_COLLECTION),
        ("command.env", command.env.len(), MAX_SMALL_COLLECTION),
        (
            "command.prerequisites",
            command.prerequisites.len(),
            MAX_SMALL_COLLECTION,
        ),
        (
            "command.prerequisite_fingerprints",
            command.prerequisite_fingerprints.len(),
            MAX_SMALL_COLLECTION,
        ),
        ("cache.inputs", cache.inputs.len(), 64),
        ("cache.outputs", cache.outputs.len(), 64),
        ("cache.env", cache.env.len(), 64),
        ("cache.runtime", cache.runtime.len(), 16),
    ])?;
    for probe in &cache.runtime {
        counts(&[("runtime.args", probe.args.len(), MAX_SMALL_COLLECTION)])?;
    }
    Ok(())
}

fn counts(collections: &[(&str, usize, usize)]) -> Result<(), RepoctlError> {
    for &(name, size, maximum) in collections {
        if size > maximum {
            return Err(plan_limit(name, maximum));
        }
    }
    Ok(())
}

fn plan_limit(resource: &str, maximum: usize) -> RepoctlError {
    RepoctlError::diagnostic(Diagnostic::error(
        format!("adoption.plan.limit.{resource}"),
        format!("saved adoption plan exceeds {maximum} {resource}"),
    ))
}

#[derive(Debug)]
struct PlanSize {
    bytes: usize,
}

impl Write for PlanSize {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.bytes > MAX_PLAN_BYTES {
            return Err(io::Error::other("adoption plan exceeds 64 MiB"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
