//! Deterministic ownership selection and evidence-preserving reverse impact traversal.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use globset::Glob;
use repoctl_core::{
    AffectedReason, AffectedReport, Diagnostic, EdgeKind, ProjectManifest, ProjectName,
    RepoRelativePath, RepoSnapshot, TaskName,
};

use super::unique_strings;

pub(super) fn compute_affected(
    snapshot: &RepoSnapshot,
    changed_files: &[RepoRelativePath],
    requested_tasks: &[TaskName],
) -> AffectedReport {
    let mut direct = BTreeSet::new();
    let mut transitive = BTreeSet::new();
    let mut direct_workspaces = BTreeSet::new();
    let mut reasons = Vec::new();
    let mut risk_flags = Vec::new();
    let mut reviewers = BTreeSet::new();
    let mut diagnostics = Vec::new();
    for changed_file in changed_files {
        if repo_wide_change(changed_file) {
            for project in &snapshot.projects {
                direct.insert(project.name.clone());
                reasons.push(reason(changed_file, &project.name, "repo-wide change"));
            }
            continue;
        }
        if changed_file.as_str().starts_with("core-infra/") {
            risk_flags.push("core-infra".to_string());
        }
        if changed_file.as_str().contains("/iac/stacks/prod") {
            risk_flags.push("prod-iac".to_string());
        }
        let mut owned = false;
        for project in &snapshot.projects {
            if project.contains_path(changed_file) {
                owned = true;
                direct.insert(project.name.clone());
                collect_matching_workspaces(project, changed_file, &mut direct_workspaces);
                reasons.push(reason(changed_file, &project.name, "project file changed"));
                reviewers.extend(project.owners.iter().cloned());
            }
            if proto_change_affects_project(project, changed_file) {
                owned = true;
                direct.insert(project.name.clone());
                reasons.push(reason(
                    changed_file,
                    &project.name,
                    "proto ownership or consumption",
                ));
                reviewers.extend(project.owners.iter().cloned());
            }
        }
        if !owned {
            diagnostics.push(Diagnostic::warning(
                "affected.unowned_file",
                format!("changed file `{changed_file}` has no declared owner"),
            ));
        }
    }
    propagate_reverse_dependencies(snapshot, &direct, &mut transitive, &mut reasons);
    let all_projects = direct
        .iter()
        .chain(transitive.iter())
        .cloned()
        .collect::<BTreeSet<_>>();
    for project in &snapshot.projects {
        if all_projects.contains(&project.name) {
            reviewers.extend(project.owners.iter().cloned());
        }
    }
    let workspaces = affected_workspaces(snapshot, &all_projects, &direct_workspaces);
    let tasks = affected_tasks(snapshot, &all_projects, &workspaces, requested_tasks);
    AffectedReport {
        directly_affected: direct.into_iter().collect(),
        transitively_affected: transitive.into_iter().collect(),
        workspaces,
        tasks,
        risk_flags: unique_strings(risk_flags),
        reasons,
        suggested_reviewers: reviewers.into_iter().collect(),
        diagnostics,
    }
}

fn repo_wide_change(changed_file: &RepoRelativePath) -> bool {
    changed_file.as_str() == "repo.yaml"
        || changed_file.as_str().starts_with("templates/")
        || changed_file.as_str().starts_with(".agents/skills/")
        || changed_file.as_str().starts_with(".claude/skills/")
        || changed_file.as_str().starts_with(".github/")
        || matches!(
            changed_file.as_str(),
            "AGENTS.md"
                | "CLAUDE.md"
                | "Cargo.lock"
                | "Cargo.toml"
                | "package.json"
                | "package-lock.json"
                | "pnpm-lock.yaml"
                | "bun.lock"
                | "yarn.lock"
                | "uv.lock"
                | "pyproject.toml"
        )
}

fn reason(path: &RepoRelativePath, project: &ProjectName, message: &str) -> AffectedReason {
    AffectedReason {
        edge_kind: None,
        evidence: None,
        source: path.to_string(),
        target: project.to_string(),
        reason: message.to_string(),
    }
}

fn proto_change_affects_project(
    project: &ProjectManifest,
    changed_file: &RepoRelativePath,
) -> bool {
    project
        .protos
        .owns
        .iter()
        .chain(project.protos.consumes.iter())
        .any(|pattern| {
            Glob::new(pattern.as_str())
                .is_ok_and(|glob| glob.compile_matcher().is_match(changed_file.as_str()))
        })
}

fn propagate_reverse_dependencies(
    snapshot: &RepoSnapshot,
    direct: &BTreeSet<ProjectName>,
    transitive: &mut BTreeSet<ProjectName>,
    reasons: &mut Vec<AffectedReason>,
) {
    let mut reverse = BTreeMap::new();
    for edge in &snapshot.graph.edges {
        if !matches!(
            edge.kind,
            EdgeKind::DependsOnProject
                | EdgeKind::UsesFrameworkFacade
                | EdgeKind::UsesFoundationClient
                | EdgeKind::ConsumesProto
                | EdgeKind::UsesFrameworkInternal
                | EdgeKind::UsesFoundationInternal
                | EdgeKind::UsesCoreInfraModule
                | EdgeKind::UsesCoreInfraInternalModule
        ) {
            continue;
        }
        let (Some(source), Some(target)) =
            (project_from_node(&edge.from), project_from_node(&edge.to))
        else {
            continue;
        };
        let (Ok(source), Ok(target)) = (ProjectName::new(source), ProjectName::new(target)) else {
            continue;
        };
        reverse
            .entry(target)
            .or_insert_with(Vec::new)
            .push((source, edge));
    }
    let mut pending = VecDeque::from_iter(direct.iter().cloned());
    while let Some(target) = pending.pop_front() {
        for (source, edge) in reverse.get(&target).into_iter().flatten() {
            if direct.contains(source) || !transitive.insert(source.clone()) {
                continue;
            }
            reasons.push(AffectedReason {
                source: target.to_string(),
                target: source.to_string(),
                reason: "reverse dependency propagation".to_string(),
                edge_kind: Some(edge.kind.clone()),
                evidence: edge.evidence.clone(),
            });
            pending.push_back(source.clone());
        }
    }
}

fn project_from_node(node: &str) -> Option<String> {
    node.strip_prefix("project:").map(ToString::to_string)
}

fn collect_matching_workspaces(
    project: &ProjectManifest,
    changed_file: &RepoRelativePath,
    direct_workspaces: &mut BTreeSet<String>,
) {
    let mut matched = false;
    for workspace in &project.workspaces {
        if project
            .path
            .join_project(&workspace.root)
            .is_ok_and(|root| changed_file.starts_with(&root))
        {
            direct_workspaces.insert(format!("{}:{}", project.name, workspace.name));
            matched = true;
        }
    }
    if !matched {
        for workspace in &project.workspaces {
            direct_workspaces.insert(format!("{}:{}", project.name, workspace.name));
        }
    }
}

fn affected_workspaces(
    snapshot: &RepoSnapshot,
    projects: &BTreeSet<ProjectName>,
    direct_workspaces: &BTreeSet<String>,
) -> Vec<String> {
    let mut workspaces = Vec::new();
    for project in &snapshot.projects {
        if !projects.contains(&project.name) {
            continue;
        }
        for workspace in &project.workspaces {
            let id = format!("{}:{}", project.name, workspace.name);
            let has_specific_workspaces = direct_workspaces
                .iter()
                .any(|workspace| workspace.starts_with(&format!("{}:", project.name)));
            if direct_workspaces.is_empty()
                || direct_workspaces.contains(&id)
                || !has_specific_workspaces
            {
                workspaces.push(id);
            }
        }
    }
    workspaces
}

fn affected_tasks(
    snapshot: &RepoSnapshot,
    projects: &BTreeSet<ProjectName>,
    workspaces: &[String],
    requested_tasks: &[TaskName],
) -> Vec<String> {
    let workspace_set = workspaces.iter().cloned().collect::<BTreeSet<_>>();
    let mut tasks = BTreeSet::new();
    for project in &snapshot.projects {
        if !projects.contains(&project.name) {
            continue;
        }
        for (task, commands) in &project.tasks {
            if !requested_tasks.is_empty() && !requested_tasks.contains(task) {
                continue;
            }
            for command in commands {
                let workspace_id = format!("{}:{}", project.name, command.workspace);
                if !workspace_set.is_empty() && !workspace_set.contains(&workspace_id) {
                    continue;
                }
                tasks.insert(format!("{}:{}:{}", project.name, command.workspace, task));
            }
        }
    }
    tasks.into_iter().collect()
}
