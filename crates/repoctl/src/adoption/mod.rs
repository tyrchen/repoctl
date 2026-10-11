//! General repository adoption, dependency rewriting and reviewed file operations.

use std::{collections::BTreeMap, fs, path::Path};

use camino::{Utf8Path, Utf8PathBuf};
use repoctl_core::{
    AdoptedSource, AdoptionCiMode, AdoptionFileOperation, AdoptionPlan, AdoptionPlanRequest,
    DependencyRewriteMode, Diagnostic, OwnerHandle, ProcessCommand, ProjectManifestSynthesis,
    ProjectRelativePath, RepoRelativePath, RepoctlError, SourceInventory, ToolPrerequisite,
    ValidationMode, VerificationPlan,
};

use super::Repoctl;

mod digests;
mod files;
mod inventory;
mod lockfiles;
mod node;
mod packages;
mod placement;
mod plan_io;
mod rewrites;
mod rust;
mod source_walk;
mod synthesis;

pub(super) use files::apply_adoption_plan;
use files::copy_operations_for_source;
use inventory::{inventory_source, source_name, source_repositories};
use packages::build_adopted_package_index;
use placement::{PlacementDecision, infer_placement};
pub(super) use plan_io::read_adoption_plan;
use rewrites::{apply_rewrites_to_operations, plan_dependency_rewrites};
use source_walk::{MAX_FILES, SourceBudgets, limit};
use synthesis::{project_name_for_destination, synthesize_project_manifest};

const ADOPTION_EXCLUDED_DIRS: &[&str] = &[
    ".git",
    ".github",
    "target",
    "node_modules",
    "dist",
    ".next",
    ".turbo",
    ".cache",
    ".repoctl",
    ".venv",
];

pub(super) fn utf8_absolute(path: &Path) -> Result<Utf8PathBuf, RepoctlError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| RepoctlError::Environment(format!("failed to read cwd: {source}")))?
            .join(path)
    };
    Utf8PathBuf::from_path_buf(absolute).map_err(|path| {
        RepoctlError::diagnostic(Diagnostic::error(
            "path.non_utf8",
            format!("path is not valid UTF-8: {}", path.display()),
        ))
    })
}

fn canonical_root(path: &Path) -> Result<Utf8PathBuf, RepoctlError> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| RepoctlError::io(path.display().to_string(), error))?;
    Utf8PathBuf::from_path_buf(canonical).map_err(|path| {
        RepoctlError::diagnostic(Diagnostic::error(
            "path.non_utf8",
            format!("path is not valid UTF-8: {}", path.display()),
        ))
    })
}

pub(super) fn plan_adoption(
    request: &AdoptionPlanRequest,
    repoctl: &Repoctl,
) -> Result<AdoptionPlan, RepoctlError> {
    let source_root = canonical_root(&request.source)?;
    let dest_root = utf8_absolute(&request.dest)?;
    if !dest_root.join("repo.yaml").is_file() {
        return Err(RepoctlError::diagnostic(Diagnostic::error(
            "adoption.dest.uninitialized",
            "destination must be an initialized repoctl monorepo with repo.yaml",
        )));
    }
    let dest_root = canonical_root(dest_root.as_std_path())?;
    if source_root == dest_root {
        return Err(RepoctlError::diagnostic(Diagnostic::error(
            "adoption.source.destination_overlap",
            "source and destination must be different directories",
        )));
    }

    let mut source_plan = plan_adoption_sources(request, &source_root, &dest_root)?;

    let dependency_rewrites = if request.rewrite_deps == DependencyRewriteMode::Off {
        Vec::new()
    } else {
        let package_index = build_adopted_package_index(&source_plan.sources)?;
        plan_dependency_rewrites(&source_plan.sources, &package_index)?
    };
    if request.rewrite_deps == DependencyRewriteMode::Auto {
        apply_rewrites_to_operations(&mut source_plan.operations, &dependency_rewrites)?;
    }
    let lockfile_refreshes = if request.rewrite_deps == DependencyRewriteMode::Auto {
        lockfiles::plan_refreshes(
            &dest_root,
            &source_plan.sources,
            &dependency_rewrites,
            &mut source_plan.operations,
            &mut source_plan.manifest_syntheses,
            &mut source_plan.diagnostics,
        )?
    } else {
        Vec::new()
    };
    let ci_operations = if request.ci == AdoptionCiMode::Off {
        Vec::new()
    } else {
        let mut snapshot = repoctl.discover(repoctl_core::DiscoverRequest {
            repo: Some(dest_root.as_std_path().to_path_buf()),
        })?;
        for synthesis in &source_plan.manifest_syntheses {
            let project = repoctl_core::YamlManifestParser.parse_project_bytes(
                synthesis.manifest_path.as_str(),
                synthesis.content.as_bytes(),
            )?;
            snapshot.projects.push(project);
        }
        let content = super::ci::render_github_actions_workflow(&snapshot);
        vec![repoctl_core::FileOperation {
            path: RepoRelativePath::new(".github/workflows/repoctl.yml")
                .map_err(RepoctlError::diagnostic)?,
            operation: if request.ci == AdoptionCiMode::ReportOnly {
                "report-file"
            } else {
                "write-file"
            }
            .to_string(),
            content: Some(content),
        }]
    };
    let verification = adoption_verification_plan(
        &dest_root,
        request.verification.clone(),
        &source_plan.sources,
        &source_plan.manifest_syntheses,
    )?;

    let plan = AdoptionPlan {
        source_root,
        dest_root,
        sources: source_plan.sources,
        operations: source_plan.operations,
        dependency_rewrites,
        lockfile_refreshes,
        manifest_syntheses: source_plan.manifest_syntheses,
        ci_operations,
        verification,
        diagnostics: source_plan.diagnostics,
    };
    plan_io::validate_plan_limits(&plan)?;
    files::validate_operation_paths(&plan)?;
    Ok(plan)
}

#[derive(Debug, Default)]
struct AdoptionSourcePlan {
    sources: Vec<AdoptedSource>,
    operations: Vec<AdoptionFileOperation>,
    manifest_syntheses: Vec<ProjectManifestSynthesis>,
    diagnostics: Vec<Diagnostic>,
    budgets: SourceBudgets,
}

fn plan_adoption_sources(
    request: &AdoptionPlanRequest,
    source_root: &Utf8Path,
    dest_root: &Utf8Path,
) -> Result<AdoptionSourcePlan, RepoctlError> {
    let candidates = source_repositories(source_root, dest_root)?;
    let mut plan = AdoptionSourcePlan::default();
    for source_path in candidates {
        append_adoption_source(request, dest_root, source_path, &mut plan)?;
    }
    Ok(plan)
}

fn append_adoption_source(
    request: &AdoptionPlanRequest,
    dest_root: &Utf8Path,
    source_path: Utf8PathBuf,
    plan: &mut AdoptionSourcePlan,
) -> Result<(), RepoctlError> {
    let name = source_name(&source_path)?;
    let include_selected = request.include.is_empty() || request.include.contains(&name);
    let skipped = !include_selected || request.exclude.contains(&name);
    let inventory = if skipped {
        SourceInventory {
            name: name.clone(),
            ..SourceInventory::default()
        }
    } else {
        inventory_source(&source_path, &name, dest_root, &mut plan.budgets.inventory)?
    };
    let placement = infer_placement(
        &name,
        &inventory,
        request.map.get(&name).cloned(),
        request.kind.get(&name).cloned(),
    );
    if skipped {
        plan.sources.push(adopted_source(
            name,
            source_path,
            placement,
            inventory,
            true,
        ));
        return Ok(());
    }
    if placement.confidence < 0.75 && !placement.override_applied {
        plan.diagnostics.push(
            Diagnostic::error(
                "adoption.placement.low_confidence",
                format!(
                    "source `{name}` placement confidence {:.2} requires --map",
                    placement.confidence
                ),
            )
            .with_help(format!(
                "pass --map {name}=<lane>/{name} and optionally --kind {name}=<kind>"
            )),
        );
    }
    let owner = request
        .owner
        .get(&name)
        .cloned()
        .or_else(|| OwnerHandle::new(format!("@{name}")).ok());
    let manifest =
        synthesize_project_manifest(&source_path, &placement, &inventory, owner.as_ref())?;
    let copies = copy_operations_for_source(
        &source_path,
        &placement.destination,
        dest_root,
        &mut plan.budgets.copy,
    )?;
    if plan
        .operations
        .len()
        .saturating_add(copies.len())
        .saturating_add(1)
        > MAX_FILES
    {
        return Err(limit("operations", dest_root));
    }
    plan.operations.extend(copies);
    let manifest_path = placement
        .destination
        .join_project(&ProjectRelativePath::new("project.yaml").map_err(RepoctlError::diagnostic)?)
        .map_err(RepoctlError::diagnostic)?;
    validate_manifest_destination(dest_root, &placement.destination, &plan.operations)?;
    plan.operations.push(AdoptionFileOperation {
        id: format!("adoption.manifest.{name}"),
        operation: "write-file".to_string(),
        source_path: None,
        destination_path: manifest_path.clone(),
        content: Some(manifest.clone()),
        checksum: None,
    });
    plan.manifest_syntheses.push(ProjectManifestSynthesis {
        source: name.clone(),
        project: project_name_for_destination(&placement.destination, &placement.kind)?,
        manifest_path,
        content: manifest,
    });
    plan.sources.push(adopted_source(
        name,
        source_path,
        placement,
        inventory,
        false,
    ));
    Ok(())
}

fn validate_manifest_destination(
    dest_root: &Utf8Path,
    destination: &RepoRelativePath,
    operations: &[AdoptionFileOperation],
) -> Result<(), RepoctlError> {
    for name in ["project.yaml", "project.yml"] {
        let path = destination.as_str().to_string() + "/" + name;
        if dest_root.join(&path).exists()
            || operations
                .iter()
                .any(|op| op.destination_path.as_str() == path)
        {
            return Err(RepoctlError::diagnostic(Diagnostic::error(
                "adoption.copy.conflict",
                format!("destination `{path}` already exists or is planned twice"),
            )));
        }
    }
    Ok(())
}

fn adopted_source(
    name: String,
    source_path: Utf8PathBuf,
    placement: PlacementDecision,
    inventory: SourceInventory,
    skipped: bool,
) -> AdoptedSource {
    AdoptedSource {
        name,
        source_path,
        destination_path: placement.destination,
        inferred_kind: placement.kind,
        confidence: placement.confidence,
        reasons: placement.reasons,
        inventory,
        skipped,
        override_applied: placement.override_applied,
    }
}

fn adoption_verification_plan(
    dest_root: &Utf8Path,
    mode: ValidationMode,
    sources: &[AdoptedSource],
    syntheses: &[ProjectManifestSynthesis],
) -> Result<VerificationPlan, RepoctlError> {
    let mut prerequisites = BTreeMap::new();
    for source in sources.iter().filter(|source| !source.skipped) {
        for tool in &source.inventory.required_tools {
            prerequisites.insert(
                tool.clone(),
                ToolPrerequisite {
                    tool: tool.clone(),
                    reason: format!("required by adopted source `{}`", source.name),
                },
            );
        }
    }
    let repo = RepoRelativePath::root();
    let mut commands = vec![
        process_command(
            dest_root,
            &repo,
            "repoctl",
            ["graph", "validate", "--mode", "structural"],
        ),
        process_command(dest_root, &repo, "repoctl", ["hygiene", "check"]),
    ];
    if matches!(mode, ValidationMode::Metadata | ValidationMode::Full) {
        commands.push(process_command(
            dest_root,
            &repo,
            "repoctl",
            ["graph", "validate", "--mode", "metadata"],
        ));
    }
    if mode == ValidationMode::Full {
        for source in sources.iter().filter(|source| !source.skipped) {
            rust::append_verification(dest_root, source, syntheses, &mut commands)?;
            synthesis::append_node_build_verification(dest_root, source, &mut commands)?;
        }
    }
    Ok(VerificationPlan {
        prerequisites: prerequisites.into_values().collect(),
        commands,
    })
}

fn process_command<const N: usize>(
    root: &Utf8Path,
    cwd: &RepoRelativePath,
    program: &str,
    args: [&str; N],
) -> ProcessCommand {
    ProcessCommand {
        cwd: cwd.clone(),
        absolute_cwd: Some(root.join(cwd.as_str()).as_std_path().to_path_buf()),
        program: program.to_string(),
        args: args.into_iter().map(ToString::to_string).collect(),
        ..ProcessCommand::default()
    }
}
