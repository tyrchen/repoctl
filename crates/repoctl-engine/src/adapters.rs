//! Concrete local adapters.
// Local discovery ports are synchronous by design in the phase 0-3 facade.
// These calls are not used from async request handlers, so blocking std fs is explicit here.
#![allow(clippy::disallowed_methods)]

use std::{fs, path::Path};

use camino::{Utf8Path, Utf8PathBuf};
use cargo_metadata::MetadataCommand;
use ignore::{DirEntry, WalkBuilder};
use repoctl_core::{
    DependencySurface, DiscoveredEdge, EdgeKind, RepoFileSystem, RepoLocator, RepoRelativePath,
    RepoRoot, RepoctlError, WalkRequest, WorkspaceInspectionInput, WorkspaceInspector,
    WorkspaceLanguage, utf8_path_buf,
};

/// Local repo locator using `repo.yaml` first and `.git` as a fallback root marker.
#[derive(Clone, Debug, Default)]
pub struct DefaultRepoLocator;

impl RepoLocator for DefaultRepoLocator {
    fn locate(&self, start: Option<&Path>) -> Result<RepoRoot, RepoctlError> {
        let start_path = match start {
            Some(path) => path.to_path_buf(),
            None => std::env::current_dir().map_err(|source| RepoctlError::io(".", source))?,
        };
        let canonical = fs::canonicalize(&start_path)
            .map_err(|source| RepoctlError::io(pathbuf_to_utf8_lossy(&start_path), source))?;
        let mut current = if canonical.is_file() {
            canonical.parent().map(Path::to_path_buf).ok_or_else(|| {
                RepoctlError::Internal("file path has no parent directory".to_string())
            })?
        } else {
            canonical
        };
        let mut git_root = None;
        loop {
            if current.join("repo.yaml").is_file() {
                return RepoRoot::new(utf8_path_buf(current).map_err(RepoctlError::diagnostic)?)
                    .map_err(RepoctlError::diagnostic);
            }
            if current.join(".git").exists() && git_root.is_none() {
                git_root = Some(current.clone());
            }
            let Some(parent) = current.parent() else {
                break;
            };
            current = parent.to_path_buf();
        }
        if let Some(root) = git_root {
            return RepoRoot::new(utf8_path_buf(root).map_err(RepoctlError::diagnostic)?)
                .map_err(RepoctlError::diagnostic);
        }
        Err(RepoctlError::diagnostic(
            repoctl_core::Diagnostic::error(
                "repo.root.not_found",
                "could not find repo.yaml or a Git root",
            )
            .with_help("pass --repo with the repository root"),
        ))
    }
}

/// Local filesystem adapter with `.gitignore` support.
#[derive(Clone, Debug, Default)]
pub struct LocalRepoFileSystem;

impl RepoFileSystem for LocalRepoFileSystem {
    fn read_file(&self, root: &RepoRoot, path: &RepoRelativePath) -> Result<Vec<u8>, RepoctlError> {
        let absolute = root.join(path);
        fs::read(absolute.as_std_path()).map_err(|source| RepoctlError::io(absolute, source))
    }

    fn walk(
        &self,
        root: &RepoRoot,
        request: &WalkRequest,
    ) -> Result<Vec<RepoRelativePath>, RepoctlError> {
        let mut files = Vec::new();
        for prefix in &request.roots {
            let start = root.join(prefix);
            if !start.exists() {
                continue;
            }
            let mut builder = WalkBuilder::new(start.as_std_path());
            builder
                .hidden(false)
                .parents(true)
                .git_ignore(true)
                .git_exclude(true)
                .git_global(true)
                .filter_entry(|entry| !is_excluded_entry(entry));
            for entry in builder.build() {
                let entry = entry.map_err(|error| {
                    RepoctlError::Environment(format!("failed to walk repository: {error}"))
                })?;
                if !entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_file())
                {
                    continue;
                }
                let relative = strip_repo_prefix(&root.absolute, entry.path())?;
                if is_excluded_path(&relative) {
                    continue;
                }
                files.push(relative);
                if files.len() > request.max_files {
                    return Err(RepoctlError::diagnostic(repoctl_core::Diagnostic::error(
                        "repo.walk.too_many_files",
                        format!("repository walk exceeded {} files", request.max_files),
                    )));
                }
            }
        }
        files.sort();
        files.dedup();
        Ok(files)
    }
}

/// Rust workspace inspector backed by `cargo_metadata`.
#[derive(Clone, Debug, Default)]
pub struct RustWorkspaceInspector;

impl WorkspaceInspector for RustWorkspaceInspector {
    fn language(&self) -> WorkspaceLanguage {
        WorkspaceLanguage::Rust
    }

    fn inspect(
        &self,
        input: &WorkspaceInspectionInput<'_>,
    ) -> Result<Vec<DiscoveredEdge>, RepoctlError> {
        if input.workspace.language != WorkspaceLanguage::Rust {
            return Ok(Vec::new());
        }
        let manifest_relative = input
            .project
            .path
            .join_project(&input.workspace.manifest)
            .map_err(RepoctlError::diagnostic)?;
        let manifest_path = input.root.join(&manifest_relative);
        if !manifest_path.is_file() {
            return Ok(Vec::new());
        }
        let metadata = MetadataCommand::new()
            .manifest_path(manifest_path.as_std_path())
            .exec()
            .map_err(|error| {
                RepoctlError::Environment(format!(
                    "cargo metadata failed for {manifest_relative}: {error}"
                ))
            })?;
        let mut edges = Vec::new();
        for package in &metadata.packages {
            for dependency in &package.dependencies {
                let Some(path) = &dependency.path else {
                    continue;
                };
                let dependency_path = absolutize_dependency_path(&manifest_path, path);
                let relative =
                    strip_repo_prefix(&input.root.absolute, dependency_path.as_std_path())?;
                if let Some(edge) = classify_path_dependency(input, &relative) {
                    edges.push(edge);
                }
            }
        }
        edges.sort_by(|left, right| {
            (
                &left.from_project,
                &left.to_project,
                edge_rank(&left.kind),
                &left.evidence,
            )
                .cmp(&(
                    &right.from_project,
                    &right.to_project,
                    edge_rank(&right.kind),
                    &right.evidence,
                ))
        });
        edges.dedup();
        Ok(edges)
    }
}

fn classify_path_dependency(
    input: &WorkspaceInspectionInput<'_>,
    relative: &RepoRelativePath,
) -> Option<DiscoveredEdge> {
    for target in input.projects {
        if target.name == input.project.name || !relative.starts_with(&target.path) {
            continue;
        }
        let kind = match target.kind {
            repoctl_core::ProjectKind::Framework => {
                if path_matches_areas(relative, target, &target.areas.public_facades) {
                    EdgeKind::UsesFrameworkFacade
                } else {
                    EdgeKind::UsesFrameworkInternal
                }
            }
            repoctl_core::ProjectKind::FoundationService => {
                if path_matches_areas(relative, target, &target.areas.public_clients) {
                    EdgeKind::UsesFoundationClient
                } else {
                    EdgeKind::UsesFoundationInternal
                }
            }
            repoctl_core::ProjectKind::CoreInfra => {
                let surface = classify_core_infra_module(relative, target);
                match surface {
                    DependencySurface::CoreInfraPublicModule => EdgeKind::UsesCoreInfraModule,
                    DependencySurface::CoreInfraInternalModule => {
                        EdgeKind::UsesCoreInfraInternalModule
                    }
                    _ => EdgeKind::DependsOnProject,
                }
            }
            repoctl_core::ProjectKind::App | repoctl_core::ProjectKind::ProtoRoot => {
                EdgeKind::DependsOnProject
            }
        };
        return Some(DiscoveredEdge {
            from_project: input.project.name.to_string(),
            from_workspace: input.workspace.name.to_string(),
            to_project: target.name.to_string(),
            kind,
            evidence: Some(relative.to_string()),
        });
    }
    None
}

fn path_matches_areas(
    relative: &RepoRelativePath,
    target: &repoctl_core::ProjectManifest,
    areas: &std::collections::BTreeMap<String, Vec<repoctl_core::ProjectRelativePath>>,
) -> bool {
    areas.values().flatten().any(|area| {
        target
            .path
            .join_project(area)
            .is_ok_and(|repo_path| relative.starts_with(&repo_path))
    })
}

fn classify_core_infra_module(
    relative: &RepoRelativePath,
    target: &repoctl_core::ProjectManifest,
) -> DependencySurface {
    if target.areas.public_modules.iter().any(|area| {
        target
            .path
            .join_project(area)
            .is_ok_and(|repo_path| relative.starts_with(&repo_path))
    }) {
        DependencySurface::CoreInfraPublicModule
    } else if target.areas.internal_modules.iter().any(|area| {
        target
            .path
            .join_project(area)
            .is_ok_and(|repo_path| relative.starts_with(&repo_path))
    }) {
        DependencySurface::CoreInfraInternalModule
    } else {
        DependencySurface::Unspecified
    }
}

fn absolutize_dependency_path(manifest_path: &Utf8Path, dependency_path: &Utf8Path) -> Utf8PathBuf {
    if dependency_path.is_absolute() {
        dependency_path.to_path_buf()
    } else {
        manifest_path.parent().map_or_else(
            || dependency_path.to_path_buf(),
            |parent| parent.join(dependency_path),
        )
    }
}

fn strip_repo_prefix(root: &Utf8Path, path: &Path) -> Result<RepoRelativePath, RepoctlError> {
    let relative = path.strip_prefix(root.as_std_path()).map_err(|error| {
        RepoctlError::Environment(format!("walked path escaped repository root: {error}"))
    })?;
    let relative = utf8_path_buf(relative.to_path_buf()).map_err(RepoctlError::diagnostic)?;
    RepoRelativePath::new(relative.as_str().to_string()).map_err(RepoctlError::diagnostic)
}

fn is_excluded_entry(entry: &DirEntry) -> bool {
    entry
        .path()
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, ".git" | "target"))
}

fn is_excluded_path(path: &RepoRelativePath) -> bool {
    let value = path.as_str();
    value.starts_with("target/")
        || value == "target"
        || value.starts_with(".git/")
        || value == ".git"
        || value.starts_with(".repoctl/cache/")
}

fn pathbuf_to_utf8_lossy(path: &Path) -> Utf8PathBuf {
    Utf8PathBuf::from(path.to_string_lossy().to_string())
}

fn edge_rank(kind: &EdgeKind) -> u8 {
    match kind {
        EdgeKind::DependsOnProject => 0,
        EdgeKind::ContainsWorkspace => 1,
        EdgeKind::ConsumesProto => 2,
        EdgeKind::OwnsProto => 3,
        EdgeKind::UsesFrameworkFacade => 4,
        EdgeKind::UsesFrameworkInternal => 5,
        EdgeKind::UsesFoundationClient => 6,
        EdgeKind::UsesFoundationInternal => 7,
        EdgeKind::UsesCoreInfraModule => 8,
        EdgeKind::UsesCoreInfraInternalModule => 9,
        EdgeKind::OwnsIac => 10,
        EdgeKind::RunsTask => 11,
    }
}
