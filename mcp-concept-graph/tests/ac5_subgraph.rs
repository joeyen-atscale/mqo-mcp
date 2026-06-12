// AC5: subgraph(ids) returns a ConceptGraph containing only the specified
// nodes and the edges between them.

use mcp_concept_graph::{ConceptGraph, Edge, EdgeKind, Node, NodeKind};

fn build_full_graph() -> ConceptGraph {
    // A → B → C
    // A → D (D has no connection to B or C)
    let mut g = ConceptGraph::new();
    for id in ["A", "B", "C", "D"] {
        g.add_node(Node::new(id, id, NodeKind::Measure));
    }
    g.add_edge(Edge::new("A", "B", EdgeKind::RelatedTo));
    g.add_edge(Edge::new("B", "C", EdgeKind::RelatedTo));
    g.add_edge(Edge::new("A", "D", EdgeKind::RelatedTo));
    g
}

#[test]
fn test_subgraph_nodes() {
    let g = build_full_graph();
    let sub = g.subgraph(&["A", "B", "C"]);
    let ids: Vec<_> = sub.nodes().iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids.len(), 3, "expected 3 nodes, got {:?}", ids);
    assert!(ids.contains(&"A"));
    assert!(ids.contains(&"B"));
    assert!(ids.contains(&"C"));
    assert!(!ids.contains(&"D"), "D should not appear in subgraph");
}

#[test]
fn test_subgraph_edges_within_set() {
    let g = build_full_graph();
    let sub = g.subgraph(&["A", "B", "C"]);
    // A→B and B→C should be present; A→D should not.
    let edges = sub.edges();
    assert_eq!(edges.len(), 2, "expected 2 edges, got {}", edges.len());
    assert!(sub.edges_from("A").iter().any(|e| e.to == "B"));
    assert!(sub.edges_from("B").iter().any(|e| e.to == "C"));
}

#[test]
fn test_subgraph_excludes_crossing_edges() {
    let g = build_full_graph();
    // Subgraph {A, D} — the A→D edge is inside, A→B is excluded (B not in set).
    let sub = g.subgraph(&["A", "D"]);
    assert_eq!(sub.nodes().len(), 2);
    assert_eq!(sub.edges().len(), 1);
    assert!(sub.edges_from("A").iter().any(|e| e.to == "D"));
    assert!(sub.edges_from("A").iter().all(|e| e.to != "B"));
}

#[test]
fn test_subgraph_singleton() {
    let g = build_full_graph();
    let sub = g.subgraph(&["A"]);
    assert_eq!(sub.nodes().len(), 1);
    assert_eq!(sub.edges().len(), 0);
}

#[test]
fn test_subgraph_empty() {
    let g = build_full_graph();
    let sub = g.subgraph(&[]);
    assert!(sub.nodes().is_empty());
    assert!(sub.edges().is_empty());
}
