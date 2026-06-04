//! Repository graph construction.

use std::{collections::BTreeMap, sync::Arc};

use repoctl_core::{
    DependencySurface, DependencyTarget, EdgeKind, GraphBuildInput, GraphBuilder, GraphEdge,
    GraphNode, GraphNodeKind, ProjectKind, RepoGraph, RepoctlError, WorkspaceInspectionInput,
    WorkspaceInspector,
};

use crate::RustWorkspaceInspector;

/// Default graph builder combining manifest edges and workspace inspector edges.
#[derive(Clone)]
pub struct DefaultGraphBuilder {
    inspectors: Vec<Arc<dyn WorkspaceInspector>>,
}

impl std::fmt::Debug for DefaultGraphBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultGraphBuilder")
            .field("inspectors", &self.inspectors.len())
            .finish()
    }
}

impl Default for DefaultGraphBuilder {
    fn default() -> Self {
        Self {
            inspectors: vec![Arc::new(RustWorkspaceInspector)],
        }
    }
}

impl DefaultGraphBuilder {
    /// Creates a graph builder from explicit workspace inspectors.
    pub fn new(inspectors: Vec<Arc<dyn WorkspaceInspector>>) -> Self {
        Self { inspectors }
    }
}

impl GraphBuilder for DefaultGraphBuilder {
    fn build(&self, input: GraphBuildInput) -> Result<RepoGraph, RepoctlError> {
        let mut graph = RepoGraph::default();
        let by_name = input
            .projects
            .iter()
            .map(|project| (project.name.clone(), project))
            .collect::<BTreeMap<_, _>>();
        for project in &input.projects {
            graph.add_node(GraphNode {
                id: project.node_id(),
                kind: GraphNodeKind::Project,
                label: project.name.to_string(),
                project: Some(project.name.clone()),
                workspace: None,
            });
            for workspace in &project.workspaces {
                let workspace_node = format!("workspace:{}:{}", project.name, workspace.name);
                graph.add_node(GraphNode {
                    id: workspace_node.clone(),
                    kind: GraphNodeKind::Workspace,
                    label: workspace.name.to_string(),
                    project: Some(project.name.clone()),
                    workspace: Some(workspace.name.clone()),
                });
                graph.add_edge(GraphEdge {
                    from: project.node_id(),
                    to: workspace_node,
                    kind: EdgeKind::ContainsWorkspace,
                    evidence: Some(workspace.manifest.to_string()),
                });
            }
            add_proto_edges(&mut graph, project);
            add_iac_edges(&mut graph, project);
        }
        for project in &input.projects {
            for dependency in &project.depends_on {
                match &dependency.target {
                    DependencyTarget::Project(target_name) => {
                        if let Some(target) = by_name.get(target_name) {
                            graph.add_edge(GraphEdge {
                                from: project.node_id(),
                                to: target.node_id(),
                                kind: classify_declared_dependency(
                                    &project.kind,
                                    &target.kind,
                                    &dependency.surface,
                                ),
                                evidence: Some(dependency.id.clone()),
                            });
                        }
                    }
                    DependencyTarget::ProtoPackage(package) => {
                        let node_id = format!("proto:{package}");
                        graph.add_node(GraphNode {
                            id: node_id.clone(),
                            kind: GraphNodeKind::ProtoPackage,
                            label: package.to_string(),
                            project: None,
                            workspace: None,
                        });
                        graph.add_edge(GraphEdge {
                            from: project.node_id(),
                            to: node_id,
                            kind: EdgeKind::ConsumesProto,
                            evidence: Some(dependency.id.clone()),
                        });
                    }
                }
            }
        }
        for project in &input.projects {
            for workspace in &project.workspaces {
                for inspector in &self.inspectors {
                    if inspector.language() != workspace.language {
                        continue;
                    }
                    let inspection = WorkspaceInspectionInput {
                        root: &input.root,
                        projects: &input.projects,
                        project,
                        workspace,
                    };
                    for discovered in inspector.inspect(&inspection)? {
                        graph.add_edge(repoctl_core::discovered_to_graph_edge(&discovered));
                    }
                }
            }
        }
        graph.nodes.sort_by(|left, right| left.id.cmp(&right.id));
        graph.edges.sort_by(|left, right| {
            (&left.from, &left.to, edge_rank(&left.kind), &left.evidence).cmp(&(
                &right.from,
                &right.to,
                edge_rank(&right.kind),
                &right.evidence,
            ))
        });
        Ok(graph)
    }
}

fn add_proto_edges(graph: &mut RepoGraph, project: &repoctl_core::ProjectManifest) {
    for pattern in &project.protos.owns {
        let node_id = format!("proto:{}", pattern.as_str());
        graph.add_node(GraphNode {
            id: node_id.clone(),
            kind: GraphNodeKind::ProtoPackage,
            label: pattern.to_string(),
            project: None,
            workspace: None,
        });
        graph.add_edge(GraphEdge {
            from: project.node_id(),
            to: node_id,
            kind: EdgeKind::OwnsProto,
            evidence: Some(pattern.to_string()),
        });
    }
    for pattern in &project.protos.consumes {
        let node_id = format!("proto:{}", pattern.as_str());
        graph.add_node(GraphNode {
            id: node_id.clone(),
            kind: GraphNodeKind::ProtoPackage,
            label: pattern.to_string(),
            project: None,
            workspace: None,
        });
        graph.add_edge(GraphEdge {
            from: project.node_id(),
            to: node_id,
            kind: EdgeKind::ConsumesProto,
            evidence: Some(pattern.to_string()),
        });
    }
}

fn add_iac_edges(graph: &mut RepoGraph, project: &repoctl_core::ProjectManifest) {
    let Some(iac) = &project.iac else {
        return;
    };
    if iac.stacks.is_empty() {
        add_iac_target(graph, project, "default", &iac.root);
        return;
    }
    for stack in &iac.stacks {
        add_iac_target(graph, project, stack, &iac.root);
    }
}

fn add_iac_target(
    graph: &mut RepoGraph,
    project: &repoctl_core::ProjectManifest,
    stack: &str,
    root: &repoctl_core::ProjectRelativePath,
) {
    let node_id = format!("iac:{}:{stack}", project.name);
    graph.add_node(GraphNode {
        id: node_id.clone(),
        kind: GraphNodeKind::IacTarget,
        label: stack.to_string(),
        project: Some(project.name.clone()),
        workspace: None,
    });
    graph.add_edge(GraphEdge {
        from: project.node_id(),
        to: node_id,
        kind: EdgeKind::OwnsIac,
        evidence: Some(root.to_string()),
    });
}

fn classify_declared_dependency(
    source: &ProjectKind,
    target: &ProjectKind,
    surface: &DependencySurface,
) -> EdgeKind {
    match (source, target, surface) {
        (ProjectKind::App, ProjectKind::Framework, DependencySurface::FrameworkInternal) => {
            EdgeKind::UsesFrameworkInternal
        }
        (ProjectKind::App, ProjectKind::Framework, _) => EdgeKind::UsesFrameworkFacade,
        (
            ProjectKind::App,
            ProjectKind::FoundationService,
            DependencySurface::FoundationPublicClient,
        ) => EdgeKind::UsesFoundationClient,
        (ProjectKind::App, ProjectKind::FoundationService, _) => EdgeKind::UsesFoundationInternal,
        (
            _,
            ProjectKind::CoreInfra | ProjectKind::CoreInfraComponent,
            DependencySurface::CoreInfraInternalModule,
        ) => EdgeKind::UsesCoreInfraInternalModule,
        (_, ProjectKind::CoreInfra | ProjectKind::CoreInfraComponent, _) => {
            EdgeKind::UsesCoreInfraModule
        }
        _ => EdgeKind::DependsOnProject,
    }
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
