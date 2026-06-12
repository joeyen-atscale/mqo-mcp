// AC7: On a 500-node graph, depth-4 traversal + probe synthesis for top 8
// completes in under 250ms.

use std::time::Instant;

use mcp_concept_graph::{ConceptGraph, Edge, EdgeKind, Node, NodeKind};
use serde_json::json;

fn build_large_graph(n_nodes: usize) -> ConceptGraph {
    let mut g = ConceptGraph::new();

    // Add root target
    let mut root = Node::new("Root", "Root", NodeKind::Measure);
    root.model_name = "bench".into();
    g.add_node(root);

    // Add n_nodes-1 more nodes forming a wide+deep tree
    // Layer 1: 10 direct children of Root
    // Layer 2: 10 children each of layer-1 nodes
    // Layer 3: remaining nodes as children of layer-2 nodes
    for i in 0..n_nodes.saturating_sub(1) {
        let name = format!("Node{i}");
        let mut n = Node::new(&name, &name, NodeKind::Measure);
        n.model_name = "bench".into();
        g.add_node(n);
    }

    // Connect: Root -> Node0..Node9 (layer 1)
    for i in 0..10.min(n_nodes.saturating_sub(1)) {
        g.add_edge(Edge::new("Root", &format!("Node{i}"), EdgeKind::DerivesFrom));
    }
    // Node0..9 -> Node10..109 (layer 2, 10 children each)
    for parent in 0..10 {
        for child_offset in 0..10 {
            let child_idx = 10 + parent * 10 + child_offset;
            if child_idx < n_nodes.saturating_sub(1) {
                g.add_edge(Edge::new(
                    &format!("Node{parent}"),
                    &format!("Node{child_idx}"),
                    EdgeKind::AggregatesVia,
                ));
            }
        }
    }
    // Node10..109 -> Node110..499 (layer 3)
    for parent in 10..110 {
        for child_offset in 0..4 {
            let child_idx = 110 + (parent - 10) * 4 + child_offset;
            if child_idx < n_nodes.saturating_sub(1) {
                g.add_edge(Edge::new(
                    &format!("Node{parent}"),
                    &format!("Node{child_idx}"),
                    EdgeKind::FiltersBy,
                ));
            }
        }
    }

    g
}

#[test]
fn test_ac7_500_node_depth4_under_250ms() {
    let graph = build_large_graph(500);
    let sa = json!({});
    let sb = json!({});

    let start = Instant::now();
    let result = mcp_hypothesis_engine::run_engine(&graph, "Root", -0.05, &sa, &sb, 4, 8);
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 250,
        "traversal took {}ms, must be < 250ms",
        elapsed.as_millis()
    );
    assert!(result.hypotheses.len() <= 8);
    assert_eq!(result.evidence_type, "structural");

    eprintln!("AC7 perf: {}ms for 500-node graph, top-8 depth-4", elapsed.as_millis());
}
