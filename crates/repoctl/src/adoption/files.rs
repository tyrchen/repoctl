//! Reviewed adoption file operations and copying.

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use repoctl_core::{
    AdoptionFileOperation, AdoptionPlan, Diagnostic, ProjectRelativePath, RepoRelativePath,
    RepoctlError, Severity,
};

use super::ADOPTION_EXCLUDED_DIRS;
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
        if let Some(content) = &operation.content {
            write_utf8_file(plan.dest_root.as_std_path(), &operation.path, content)?;
        }
    }
    Ok(())
}

fn validate_operation_paths(plan: &AdoptionPlan) -> Result<(), RepoctlError> {
    for operation in &plan.operations {
        validate_destination(plan, &operation.destination_path)?;
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
        validate_destination(plan, &operation.path)?;
    }
    Ok(())
}

fn validate_destination(
    plan: &AdoptionPlan,
    destination: &RepoRelativePath,
) -> Result<(), RepoctlError> {
    RepoRelativePath::new(destination.as_str()).map_err(RepoctlError::diagnostic)?;
    let absolute = plan.dest_root.join(destination.as_str());
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
        write_utf8_file(
            plan.dest_root.as_std_path(),
            &operation.destination_path,
            content,
        )?;
        return Ok(());
    }
    let Some(source) = &operation.source_path else {
        return Ok(());
    };
    if destination.exists() {
        return Err(RepoctlError::diagnostic(
            Diagnostic::error(
                "adoption.copy.conflict",
                format!(
                    "destination `{}` already exists",
                    operation.destination_path
                ),
            )
            .with_path(operation.destination_path.as_str()),
        ));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| RepoctlError::io(parent.as_str().to_string(), source))?;
    }
    fs::copy(source, &destination)
        .map_err(|source_error| RepoctlError::io(destination.as_str().to_string(), source_error))?;
    let metadata = fs::metadata(source)
        .map_err(|source_error| RepoctlError::io(source.as_str().to_string(), source_error))?;
    fs::set_permissions(&destination, metadata.permissions())
        .map_err(|source_error| RepoctlError::io(destination.as_str().to_string(), source_error))?;
    Ok(())
}

pub(super) fn copy_operations_for_source(
    source_path: &Utf8Path,
    destination: &RepoRelativePath,
    dest_root: &Utf8Path,
) -> Result<Vec<AdoptionFileOperation>, RepoctlError> {
    let mut operations = Vec::new();
    collect_copy_operations(
        source_path,
        source_path,
        destination,
        dest_root,
        &mut operations,
    )?;
    Ok(operations)
}

fn copy_destination(
    root: &Utf8Path,
    path: &Utf8Path,
    destination: &RepoRelativePath,
) -> Result<RepoRelativePath, RepoctlError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        RepoctlError::diagnostic(Diagnostic::error(
            "adoption.copy.path",
            "source path escaped source root",
        ))
    })?;
    let relative_project_path = ProjectRelativePath::new(relative.as_str().to_string())
        .map_err(RepoctlError::diagnostic)?;
    let destination_path = destination
        .join_project(&relative_project_path)
        .map_err(RepoctlError::diagnostic)?;
    Ok(destination_path)
}

fn collect_copy_operations(
    root: &Utf8Path,
    current: &Utf8Path,
    destination: &RepoRelativePath,
    dest_root: &Utf8Path,
    operations: &mut Vec<AdoptionFileOperation>,
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
        let Some(name) = path.file_name() else {
            continue;
        };
        if matches!(name, "project.yaml" | "project.yml") {
            continue;
        }
        if path.is_dir() && ADOPTION_EXCLUDED_DIRS.contains(&name) {
            continue;
        }
        let destination_path = copy_destination(root, &path, destination)?;
        if path.is_symlink() {
            return Err(RepoctlError::diagnostic(Diagnostic::error(
                "adoption.copy.symlink",
                format!("adoption source contains a symlink: {path}"),
            )));
        }
        if path.is_dir() {
            collect_copy_operations(root, &path, destination, dest_root, operations)?;
        } else {
            let target = dest_root.join(destination_path.as_str());
            if target.exists() {
                return Err(RepoctlError::diagnostic(
                    Diagnostic::error(
                        "adoption.copy.conflict",
                        format!("destination `{destination_path}` already exists"),
                    )
                    .with_path(destination_path.as_str()),
                ));
            }
            operations.push(AdoptionFileOperation {
                id: format!("adoption.copy.{destination_path}"),
                operation: "copy-file".to_string(),
                source_path: Some(path),
                destination_path,
                content: None,
                checksum: None,
            });
        }
    }
    Ok(())
}
