// AC3: k_hop_neighbors(id, 1) returns all nodes reachable via exactly one
// edge from id. k_hop_neighbors(id, 2) returns all nodes reachable in 1 or
// 2 hops (does NOT include the source node itself).

use mcp_concept_graph::{ConceptGraph, Edge, EdgeKind, Node, NodeKind};

fn build_chain_graph() -> ConceptGraph {
    // A → B → C → D
    let mut g = ConceptGraph::new();
    for (id, label) in [("A", "Node A"), ("B", "Node B"), ("C", "Node C"), ("D", "Node D")] {
        g.add_node(Node::new(id, label, NodeKind::Measure));
    }
    g.add_edge(Edge::new("A", "B", EdgeKind::RelatedTo));
    g.add_edge(Edge::new("B", "C", EdgeKind::RelatedTo));
    g.add_edge(Edge::new("C", "D", EdgeKind::RelatedTo));
    g
}

#[test]
fn test_k1_hop() {
    let g = build_chain_graph();
    let neighbors = g.k_hop_neighbors("A", 1);
    assert_eq!(neighbors.len(), 1);
    assert_eq!(neighbors[0].id, "B");
}

#[test]
fn test_k2_hop() {
    let g = build_chain_graph();
    let neighbors = g.k_hop_neighbors("A", 2);
    let ids: Vec<&str> = neighbors.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(ids.len(), 2, "expected B and C, got {:?}", ids);
    assert!(ids.contains(&"B"), "B missing from k=2 neighbors");
    assert!(ids.contains(&"C"), "C missing from k=2 neighbors");
}

#[test]
fn test_k3_hop() {
    let g = build_chain_graph();
    let neighbors = g.k_hop_neighbors("A", 3);
    assert_eq!(neighbors.len(), 3);
}

#[test]
fn test_source_not_included() {
    let g = build_chain_graph();
    for k in 1..=3u8 {
        let neighbors = g.k_hop_neighbors("A", k);
        assert!(
            !neighbors.iter().any(|n| n.id == "A"),
            "source A should not appear in k={} neighbors",
            k
        );
    }
}

#[test]
fn test_k0_hop_empty() {
    let g = build_chain_graph();
    assert!(g.k_hop_neighbors("A", 0).is_empty());
}

#[test]
fn test_branching_graph() {
    // A → B, A → C, B → D, C → D
    let mut g = ConceptGraph::new();
    for (id, label) in [("A", "A"), ("B", "B"), ("C", "C"), ("D", "D")] {
        g.add_node(Node::new(id, label, NodeKind::Measure));
    }
    g.add_edge(Edge::new("A", "B", EdgeKind::RelatedTo));
    g.add_edge(Edge::new("A", "C", EdgeKind::RelatedTo));
    g.add_edge(Edge::new("B", "D", EdgeKind::RelatedTo));
    g.add_edge(Edge::new("C", "D", EdgeKind::RelatedTo));

    let k1 = g.k_hop_neighbors("A", 1);
    assert_eq!(k1.len(), 2); // B and C

    let k2 = g.k_hop_neighbors("A", 2);
    assert_eq!(k2.len(), 3); // B, C, D (D appears once despite two paths)
}
