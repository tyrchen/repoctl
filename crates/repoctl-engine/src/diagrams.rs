//! Deterministic, escaped Mermaid exports for architecture and change impact.

use std::{collections::BTreeMap, fmt::Write as _};

use repoctl_core::{AffectedReport, RepoGraph};

/// Exports a validated repository graph as a Mermaid flowchart.
///
/// ```
/// assert!(repoctl_engine::graph_mermaid(&repoctl_core::RepoGraph::default()).starts_with("flowchart LR"));
/// ```
pub fn graph_mermaid(graph: &RepoGraph) -> String {
    let labels = graph
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node.label.clone()))
        .collect();
    let (mut diagram, ids) = nodes(&labels);
    let mut edges = graph.edges.iter().collect::<Vec<_>>();
    edges.sort_by_key(|edge| {
        (
            &edge.from,
            &edge.to,
            format!("{:?}", edge.kind),
            &edge.evidence,
        )
    });
    for edge in edges {
        if let (Some(from), Some(to)) = (ids.get(&edge.from), ids.get(&edge.to)) {
            let evidence = edge.evidence.as_deref().unwrap_or("");
            let _ = writeln!(
                diagram,
                "  n{from} -->|\"{}\"| n{to}",
                escape(&format!("{:?}: {evidence}", edge.kind))
            );
        }
    }
    diagram
}

/// Exports changed files, propagated projects and selected tasks as Mermaid.
///
/// ```
/// assert!(repoctl_engine::affected_mermaid(&repoctl_core::AffectedReport::default()).starts_with("flowchart LR"));
/// ```
pub fn affected_mermaid(report: &AffectedReport) -> String {
    let mut labels = BTreeMap::new();
    for reason in &report.reasons {
        labels.insert(reason.source.clone(), reason.source.clone());
        labels.insert(reason.target.clone(), reason.target.clone());
    }
    for task in &report.tasks {
        labels.insert(task.clone(), task.clone());
    }
    let (mut diagram, ids) = nodes(&labels);
    let mut reasons = report.reasons.iter().collect::<Vec<_>>();
    reasons.sort_by_key(|reason| {
        (
            &reason.source,
            &reason.target,
            &reason.reason,
            &reason.evidence,
        )
    });
    for reason in reasons {
        if let (Some(from), Some(to)) = (ids.get(&reason.source), ids.get(&reason.target)) {
            let label = format!(
                "{} {}",
                reason.reason,
                reason.evidence.as_deref().unwrap_or("")
            );
            let _ = writeln!(diagram, "  n{from} -->|\"{}\"| n{to}", escape(&label));
        }
    }
    let mut tasks = report.tasks.iter().collect::<Vec<_>>();
    tasks.sort();
    for task in tasks {
        if let Some((project, _)) = task.split_once(':')
            && let (Some(from), Some(to)) = (ids.get(project), ids.get(task))
        {
            let _ = writeln!(diagram, "  n{from} -->|selected task| n{to}");
        }
    }
    diagram
}

fn nodes(labels: &BTreeMap<String, String>) -> (String, BTreeMap<String, usize>) {
    let mut diagram = "flowchart LR\n".to_string();
    let mut ids = BTreeMap::new();
    for (id, (key, label)) in labels.iter().enumerate() {
        ids.insert(key.clone(), id);
        let _ = writeln!(diagram, "  n{id}[\"{}\"]", escape(label));
    }
    (diagram, ids)
}

fn escape(label: &str) -> String {
    let mut escaped = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | ':' | '/' | '-') {
            escaped.push(c);
        } else {
            let _ = write!(escaped, "#{};", u32::from(c));
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use repoctl_core::{EdgeKind, GraphEdge, GraphNode, GraphNodeKind, RepoGraph};

    use super::graph_mermaid;

    #[test]
    fn test_should_escape_diagram_labels_and_sort_edges() {
        let mut graph = RepoGraph::default();
        for id in ["b", "a"] {
            graph.nodes.push(GraphNode {
                id: id.to_string(),
                kind: GraphNodeKind::Project,
                label: "unsafe\"\nclick x <script> 中文".to_string(),
                project: None,
                workspace: None,
            });
        }
        for (from, to) in [("b", "a"), ("a", "b")] {
            graph.edges.push(GraphEdge {
                from: from.to_string(),
                to: to.to_string(),
                kind: EdgeKind::DependsOnProject,
                evidence: Some("|\"\n".to_string()),
            });
        }
        let first = graph_mermaid(&graph);
        graph.nodes.reverse();
        graph.edges.reverse();
        assert_eq!(first, graph_mermaid(&graph));
        assert!(!first.contains("<script>"));
        assert!(!first.contains("\nclick"));
        assert!(first.contains("#34;#10;"));
    }
}
