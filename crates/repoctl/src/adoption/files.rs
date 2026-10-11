//! Reviewed adoption file operations and copying.

// Adoption applies synchronously through the CLI facade, outside an async runtime.
#![allow(clippy::disallowed_types)]

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, Write as _},
};

use camino::Utf8Path;
use repoctl_core::{
    AdoptionFileOperation, AdoptionPlan, Diagnostic, RepoRelativePath, RepoctlError, Severity,
};

use super::source_walk::{MAX_FILES, ScanBudget, SourceEntryKind, limit, walk_source};
use crate::write_utf8_file;

pub(crate) fn apply_adoption_plan(plan: &AdoptionPlan) -> Result<(), RepoctlError> {
    validate_operation_paths(plan)?;
    let mut diagnostics = plan.diagnostics.clone();
    diagnostics.extend(plan.sources.iter().filter_map(|source| {
        if source.skipped || source.override_applied || source.confidence >= 0.75 {
            None
        } else {
            Some(
                Diagnostic::error(
                    "adoption.placement.low_confidence",
                    format!(
                        "source `{}` placement confidence {:.2} requires a reviewed override",
                        source.name, source.confidence
                    ),
                )
                .with_path(source.destination_path.as_str()),
            )
        }
    }));
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        return Err(RepoctlError::Diagnostics { diagnostics });
    }
    for operation in &plan.operations {
        apply_adoption_operation(plan, operation)?;
    }
    for operation in &plan.ci_operations {
        if operation.operation == "write-file"
            && let Some(content) = &operation.content
        {
            write_utf8_file(plan.dest_root.as_std_path(), &operation.path, content)?;
        }
    }
    Ok(())
}

pub(super) fn validate_operation_paths(plan: &AdoptionPlan) -> Result<(), RepoctlError> {
    if plan.operations.len() > MAX_FILES {
        return Err(limit("operations", &plan.dest_root));
    }
    let mut destinations = BTreeSet::new();
    let mut manifest_roots = BTreeSet::new();
    for operation in &plan.operations {
        validate_destination(plan, &operation.destination_path)?;
        let absolute = plan.dest_root.join(operation.destination_path.as_str());
        if !destinations.insert(operation.destination_path.clone()) || absolute.exists() {
            return Err(conflict(&operation.destination_path));
        }
        if matches!(absolute.file_name(), Some("project.yaml" | "project.yml"))
            && absolute.parent().is_some_and(|parent| {
                !manifest_roots.insert(parent.to_path_buf())
                    || parent.join("project.yaml").exists()
                    || parent.join("project.yml").exists()
            })
        {
            return Err(conflict(&operation.destination_path));
        }
        if let Some(source) = &operation.source_path {
            let root = fs::canonicalize(&plan.source_root)
                .map_err(|error| RepoctlError::io(plan.source_root.clone(), error))?;
            let canonical = fs::canonicalize(source)
                .map_err(|error| RepoctlError::io(source.clone(), error))?;
            if !canonical.starts_with(root) {
                return Err(RepoctlError::diagnostic(Diagnostic::error(
                    "adoption.copy.path",
                    "adoption source escaped source root",
                )));
            }
        }
    }
    for operation in &plan.ci_operations {
        if operation.path.as_str() != ".github/workflows/repoctl.yml" {
            return Err(RepoctlError::diagnostic(Diagnostic::error(
                "adoption.ci.path",
                "adoption CI updates must target .github/workflows/repoctl.yml",
            )));
        }
        if !matches!(operation.operation.as_str(), "write-file" | "report-file") {
            return Err(RepoctlError::diagnostic(Diagnostic::error(
                "adoption.ci.operation",
                "CI operations must be write-file or report-file",
            )));
        }
        if operation.operation == "report-file" {
            continue;
        }
        validate_destination(plan, &operation.path)?;
        if !destinations.insert(operation.path.clone()) {
            return Err(conflict(&operation.path));
        }
    }
    Ok(())
}

fn conflict(path: &RepoRelativePath) -> RepoctlError {
    RepoctlError::diagnostic(
        Diagnostic::error(
            "adoption.copy.conflict",
            format!("destination `{path}` already exists or is planned twice"),
        )
        .with_path(path.as_str()),
    )
}

fn validate_destination(
    plan: &AdoptionPlan,
    destination: &RepoRelativePath,
) -> Result<(), RepoctlError> {
    RepoRelativePath::new(destination.as_str()).map_err(RepoctlError::diagnostic)?;
    let absolute = plan.dest_root.join(destination.as_str());
    if absolute.parent().is_some_and(|parent| {
        parent
            .ancestors()
            .take_while(|ancestor| ancestor.starts_with(&plan.dest_root))
            .any(|ancestor| ancestor.exists() && !ancestor.is_dir())
    }) {
        return Err(conflict(destination));
    }
    if absolute
        .as_std_path()
        .ancestors()
        .take_while(|ancestor| ancestor.starts_with(plan.dest_root.as_std_path()))
        .any(|ancestor| {
            fs::symlink_metadata(ancestor).is_ok_and(|metadata| metadata.file_type().is_symlink())
        })
    {
        return Err(RepoctlError::diagnostic(Diagnostic::error(
            "adoption.copy.symlink",
            "adoption destination contains a symlink",
        )));
    }
    Ok(())
}

fn apply_adoption_operation(
    plan: &AdoptionPlan,
    operation: &AdoptionFileOperation,
) -> Result<(), RepoctlError> {
    if operation.operation == "reject-symlink" {
        return Err(RepoctlError::diagnostic(
            Diagnostic::error(
                "adoption.copy.symlink_rejected",
                format!(
                    "source symlink for `{}` is not allowed",
                    operation.destination_path
                ),
            )
            .with_path(operation.destination_path.as_str()),
        ));
    }
    let destination = plan.dest_root.join(operation.destination_path.as_str());
    if let Some(content) = &operation.content {
        create_destination(&destination)?
            .write_all(content.as_bytes())
            .map_err(|error| RepoctlError::io(destination.clone(), error))?;
        return Ok(());
    }
    let Some(source) = &operation.source_path else {
        return Ok(());
    };
    if destination.exists() {
        return Err(conflict(&operation.destination_path));
    }
    let mut source_file =
        File::open(source).map_err(|error| RepoctlError::io(source.clone(), error))?;
    io::copy(&mut source_file, &mut create_destination(&destination)?)
        .map_err(|error| RepoctlError::io(destination.clone(), error))?;
    let metadata = fs::metadata(source)
        .map_err(|source_error| RepoctlError::io(source.as_str().to_string(), source_error))?;
    fs::set_permissions(&destination, metadata.permissions())
        .map_err(|source_error| RepoctlError::io(destination.as_str().to_string(), source_error))?;
    Ok(())
}

fn create_destination(path: &Utf8Path) -> Result<File, RepoctlError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| RepoctlError::io(parent.to_path_buf(), error))?;
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| RepoctlError::io(path.to_path_buf(), error))
}

pub(super) fn copy_operations_for_source(
    source_path: &Utf8Path,
    destination: &RepoRelativePath,
    dest_root: &Utf8Path,
    budget: &mut ScanBudget,
) -> Result<Vec<AdoptionFileOperation>, RepoctlError> {
    let mut operations = Vec::new();
    walk_source(source_path, dest_root, budget, &mut |entry| {
        if entry.kind != SourceEntryKind::File
            || matches!(entry.path.file_name(), Some("project.yaml" | "project.yml"))
        {
            return Ok(());
        }
        let destination_path = destination
            .join_project(&entry.relative)
            .map_err(RepoctlError::diagnostic)?;
        if dest_root.join(destination_path.as_str()).exists() {
            return Err(conflict(&destination_path));
        }
        operations.push(AdoptionFileOperation {
            id: format!("adoption.copy.{destination_path}"),
            operation: "copy-file".to_string(),
            source_path: Some(entry.path),
            destination_path,
            content: None,
            checksum: None,
        });
        Ok(())
    })?;
    Ok(operations)
}
