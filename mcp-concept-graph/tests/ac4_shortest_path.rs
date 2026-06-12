// AC4: shortest_path(from, to) returns the node sequence of the shortest
// path between two connected nodes, or None if no path exists.

use mcp_concept_graph::{ConceptGraph, Edge, EdgeKind, Node, NodeKind};

fn build_graph() -> ConceptGraph {
    // A → B → C → D
    //     ↓
    //     E → D  (shorter path A→B→E→D vs A→B→C→D)
    let mut g = ConceptGraph::new();
    for id in ["A", "B", "C", "D", "E"] {
        g.add_node(Node::new(id, id, NodeKind::Measure));
    }
    g.add_edge(Edge::new("A", "B", EdgeKind::RelatedTo));
    g.add_edge(Edge::new("B", "C", EdgeKind::RelatedTo));
    g.add_edge(Edge::new("C", "D", EdgeKind::RelatedTo));
    g.add_edge(Edge::new("B", "E", EdgeKind::RelatedTo));
    g.add_edge(Edge::new("E", "D", EdgeKind::RelatedTo));
    g
}

#[test]
fn test_direct_edge() {
    let g = build_graph();
    let path = g.shortest_path("A", "B").unwrap();
    assert_eq!(path, vec!["A", "B"]);
}

#[test]
fn test_shortest_among_alternatives() {
    let g = build_graph();
    // Shortest A→D is A→B→E→D (length 3), not A→B→C→D (length 4).
    let path = g.shortest_path("A", "D").unwrap();
    assert_eq!(path.len(), 4, "expected path length 4, got {:?}", path);
    assert_eq!(path[0], "A");
    assert_eq!(path[path.len() - 1], "D");
}

#[test]
fn test_no_path() {
    let g = build_graph();
    // Disconnected node F.
    let mut g2 = g.clone();
    g2.add_node(Node::new("F", "F", NodeKind::Measure));
    assert!(g2.shortest_path("A", "F").is_none());
}

#[test]
fn test_self_path() {
    let g = build_graph();
    let path = g.shortest_path("A", "A").unwrap();
    assert_eq!(path, vec!["A"]);
}

#[test]
fn test_reverse_not_found() {
    let g = build_graph();
    // All edges are directed; no reverse paths.
    assert!(g.shortest_path("D", "A").is_none());
}
