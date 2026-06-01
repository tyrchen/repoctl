//! Boundary policy rules.

use std::collections::BTreeMap;

use globset::{Glob, GlobSet, GlobSetBuilder};
use repoctl_core::{
    Diagnostic, EdgeKind, PolicyContext, PolicyRule, ProjectKind, ProjectManifest,
    RepoRelativePath, RepoSnapshot, RepoctlError,
};

/// Evaluates the default v0.2 policy rule set.
pub struct PolicyEngine {
    rules: Vec<Box<dyn PolicyRule>>,
}

impl std::fmt::Debug for PolicyEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyEngine")
            .field("rules", &self.rules.len())
            .finish()
    }
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self {
            rules: vec![
                Box::new(CrossAppDependencyRule),
                Box::new(ProjectKindDependencyRule),
                Box::new(FrameworkFacadeOnlyRule),
                Box::new(FoundationPublicClientOnlyRule),
                Box::new(GeneratedCodeReadonlyRule),
                Box::new(HighRiskIacRule),
            ],
        }
    }
}

impl Clone for PolicyEngine {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl PolicyEngine {
    /// Evaluates all rules against a snapshot.
    pub fn evaluate(
        &self,
        snapshot: &RepoSnapshot,
        changed_files: &[RepoRelativePath],
    ) -> Result<Vec<Diagnostic>, RepoctlError> {
        let context = PolicyContext {
            snapshot,
            changed_files,
        };
        let mut diagnostics = Vec::new();
        for rule in &self.rules {
            diagnostics.extend(rule.evaluate(&context)?);
        }
        diagnostics.sort_by(|left, right| {
            (
                &left.code,
                &left.message,
                &left.project,
                &left.source.as_ref().map(|s| &s.path),
            )
                .cmp(&(
                    &right.code,
                    &right.message,
                    &right.project,
                    &right.source.as_ref().map(|s| &s.path),
                ))
        });
        Ok(diagnostics)
    }
}

/// Denies app-to-app dependencies.
#[derive(Clone, Debug, Default)]
pub struct CrossAppDependencyRule;

impl PolicyRule for CrossAppDependencyRule {
    fn name(&self) -> &'static str {
        "no-cross-app-dependency"
    }

    fn evaluate(&self, context: &PolicyContext<'_>) -> Result<Vec<Diagnostic>, RepoctlError> {
        let projects = projects_by_node(context.snapshot);
        let mut diagnostics = Vec::new();
        for edge in &context.snapshot.graph.edges {
            if edge.kind != EdgeKind::DependsOnProject {
                continue;
            }
            let Some(source) = projects.get(&edge.from) else {
                continue;
            };
            let Some(target) = projects.get(&edge.to) else {
                continue;
            };
            if source.kind == ProjectKind::App && target.kind == ProjectKind::App {
                diagnostics.push(policy_error(
                    "policy.cross_app_dependency",
                    format!(
                        "app `{}` must not depend on app `{}`",
                        source.name, target.name
                    ),
                    source,
                    edge.evidence.clone(),
                ));
            }
        }
        Ok(diagnostics)
    }
}

/// Denies framework and foundation dependencies on app projects.
#[derive(Clone, Debug, Default)]
pub struct ProjectKindDependencyRule;

impl PolicyRule for ProjectKindDependencyRule {
    fn name(&self) -> &'static str {
        "project-kind-dependency"
    }

    fn evaluate(&self, context: &PolicyContext<'_>) -> Result<Vec<Diagnostic>, RepoctlError> {
        let projects = projects_by_node(context.snapshot);
        let mut diagnostics = Vec::new();
        for edge in &context.snapshot.graph.edges {
            if edge.kind != EdgeKind::DependsOnProject {
                continue;
            }
            let Some(source) = projects.get(&edge.from) else {
                continue;
            };
            let Some(target) = projects.get(&edge.to) else {
                continue;
            };
            if target.kind == ProjectKind::App
                && matches!(
                    source.kind,
                    ProjectKind::Framework | ProjectKind::FoundationService
                )
            {
                diagnostics.push(policy_error(
                    "policy.project_kind_dependency",
                    format!(
                        "{:?} `{}` must not depend on app `{}`",
                        source.kind, source.name, target.name
                    ),
                    source,
                    edge.evidence.clone(),
                ));
            }
        }
        Ok(diagnostics)
    }
}

/// Denies app dependencies on framework internal areas.
#[derive(Clone, Debug, Default)]
pub struct FrameworkFacadeOnlyRule;

impl PolicyRule for FrameworkFacadeOnlyRule {
    fn name(&self) -> &'static str {
        "app-must-use-framework-facade"
    }

    fn evaluate(&self, context: &PolicyContext<'_>) -> Result<Vec<Diagnostic>, RepoctlError> {
        let projects = projects_by_node(context.snapshot);
        let mut diagnostics = Vec::new();
        for edge in &context.snapshot.graph.edges {
            if edge.kind != EdgeKind::UsesFrameworkInternal {
                continue;
            }
            let Some(source) = projects.get(&edge.from) else {
                continue;
            };
            if source.kind != ProjectKind::App {
                continue;
            }
            diagnostics.push(policy_error(
                "policy.framework_internal_dependency",
                format!(
                    "app `{}` must depend on framework facades only",
                    source.name
                ),
                source,
                edge.evidence.clone(),
            ));
        }
        Ok(diagnostics)
    }
}

/// Denies app dependencies on foundation internals.
#[derive(Clone, Debug, Default)]
pub struct FoundationPublicClientOnlyRule;

impl PolicyRule for FoundationPublicClientOnlyRule {
    fn name(&self) -> &'static str {
        "app-must-use-foundation-client"
    }

    fn evaluate(&self, context: &PolicyContext<'_>) -> Result<Vec<Diagnostic>, RepoctlError> {
        let projects = projects_by_node(context.snapshot);
        let mut diagnostics = Vec::new();
        for edge in &context.snapshot.graph.edges {
            if edge.kind != EdgeKind::UsesFoundationInternal {
                continue;
            }
            let Some(source) = projects.get(&edge.from) else {
                continue;
            };
            if source.kind != ProjectKind::App {
                continue;
            }
            diagnostics.push(
                policy_error(
                    "policy.foundation_internal_dependency",
                    format!(
                        "app `{}` must use foundation public clients instead of internals",
                        source.name
                    ),
                    source,
                    edge.evidence.clone(),
                )
                .with_help("depend on foundations.<service>.client"),
            );
        }
        Ok(diagnostics)
    }
}

/// Denies direct edits to generated code paths.
#[derive(Clone, Debug, Default)]
pub struct GeneratedCodeReadonlyRule;

impl PolicyRule for GeneratedCodeReadonlyRule {
    fn name(&self) -> &'static str {
        "generated-code-readonly"
    }

    fn evaluate(&self, context: &PolicyContext<'_>) -> Result<Vec<Diagnostic>, RepoctlError> {
        let generated = compile_globs(["**/generated/**"])?;
        let mut diagnostics = Vec::new();
        for changed_file in context.changed_files {
            let global_match = generated.is_match(changed_file.as_str());
            let project_match = context
                .snapshot
                .projects
                .iter()
                .any(|project| project_generated_match(project, changed_file));
            if global_match || project_match {
                diagnostics.push(
                    Diagnostic::error(
                        "policy.generated_code_readonly",
                        format!("generated file `{changed_file}` must not be edited directly"),
                    )
                    .with_path(changed_file.as_str()),
                );
            }
        }
        Ok(diagnostics)
    }
}

/// Reports high-risk infrastructure changes.
#[derive(Clone, Debug, Default)]
pub struct HighRiskIacRule;

impl PolicyRule for HighRiskIacRule {
    fn name(&self) -> &'static str {
        "high-risk-iac"
    }

    fn evaluate(&self, context: &PolicyContext<'_>) -> Result<Vec<Diagnostic>, RepoctlError> {
        let high_risk = compile_globs([
            "core-infra/**",
            "apps/*/iac/stacks/prod/**",
            "apps/*/iac/stacks/prod*",
            "foundations/*/iac/stacks/prod/**",
            "foundations/*/iac/stacks/prod*",
        ])?;
        let mut diagnostics = Vec::new();
        for changed_file in context.changed_files {
            if !high_risk.is_match(changed_file.as_str()) {
                continue;
            }
            let reviewers = required_reviewers(context.snapshot, changed_file);
            diagnostics.push(
                Diagnostic::warning(
                    "policy.high_risk_iac",
                    format!("high-risk infrastructure change `{changed_file}` requires review"),
                )
                .with_path(changed_file.as_str())
                .with_help(format!("required reviewers: {}", reviewers.join(", "))),
            );
        }
        Ok(diagnostics)
    }
}

fn projects_by_node(snapshot: &RepoSnapshot) -> BTreeMap<String, &ProjectManifest> {
    snapshot
        .projects
        .iter()
        .map(|project| (project.node_id(), project))
        .collect()
}

fn policy_error(
    code: &'static str,
    message: String,
    project: &ProjectManifest,
    evidence: Option<String>,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::error(code, message)
        .with_path(project.source.as_str())
        .with_project(project.name.as_str());
    if let Some(evidence) = evidence {
        diagnostic = diagnostic.with_help(format!("offending dependency: {evidence}"));
    }
    diagnostic
}

fn compile_globs<const N: usize>(patterns: [&str; N]) -> Result<GlobSet, RepoctlError> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|error| {
            RepoctlError::Internal(format!("invalid built-in glob `{pattern}`: {error}"))
        })?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|error| RepoctlError::Internal(format!("failed to build glob set: {error}")))
}

fn project_generated_match(project: &ProjectManifest, changed_file: &RepoRelativePath) -> bool {
    if !changed_file.starts_with(&project.path) {
        return false;
    }
    project.ai.do_not_edit.iter().any(|pattern| {
        let full_pattern = if pattern.as_str().starts_with("**/") {
            pattern.as_str().to_string()
        } else {
            format!("{}/{}", project.path, pattern.as_str())
        };
        Glob::new(&full_pattern)
            .map(|glob| glob.compile_matcher())
            .is_ok_and(|matcher| matcher.is_match(changed_file.as_str()))
    })
}

fn required_reviewers(snapshot: &RepoSnapshot, changed_file: &RepoRelativePath) -> Vec<String> {
    let mut reviewers = snapshot
        .repo_manifest
        .policies
        .prod_change_required_owners
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    for project in &snapshot.projects {
        if project.contains_path(changed_file) {
            reviewers.extend(project.owners.iter().map(ToString::to_string));
        }
    }
    reviewers.sort();
    reviewers.dedup();
    if reviewers.is_empty() {
        reviewers.push("@platform".to_string());
    }
    reviewers
}
