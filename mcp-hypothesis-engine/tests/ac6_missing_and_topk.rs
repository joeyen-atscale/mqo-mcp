// AC6: --target absent from graph → no hypotheses path tested via engine API
// (the CLI exit-1 behavior is integration-tested separately if needed).
// --top-k 3 returns at most 3 hypotheses.

use mcp_concept_graph::{ConceptGraph, Edge, EdgeKind, Node, NodeKind};
use serde_json::json;

fn build_graph_many_components() -> ConceptGraph {
    let mut g = ConceptGraph::new();
    let mut t = Node::new("Target", "Target", NodeKind::Measure);
    t.model_name = "test".into();
    g.add_node(t);
    for i in 0..10 {
        let name = format!("Comp{i}");
        let mut n = Node::new(&name, &name, NodeKind::Measure);
        n.model_name = "test".into();
        g.add_node(n);
        g.add_edge(Edge::new("Target", &name, EdgeKind::DerivesFrom));
    }
    g
}

#[test]
fn test_ac6_top_k_limits_output() {
    let graph = build_graph_many_components();
    let sa = json!({});
    let sb = json!({});

    let result = mcp_hypothesis_engine::run_engine(&graph, "Target", -0.05, &sa, &sb, 4, 3);

    assert!(
        result.hypotheses.len() <= 3,
        "top-k=3 must return at most 3 hypotheses, got {}",
        result.hypotheses.len()
    );
}

#[test]
fn test_ac6_top_k_1() {
    let graph = build_graph_many_components();
    let sa = json!({});
    let sb = json!({});

    let result = mcp_hypothesis_engine::run_engine(&graph, "Target", 0.1, &sa, &sb, 4, 1);
    assert_eq!(result.hypotheses.len(), 1);
    assert_eq!(result.hypotheses[0].rank, 1);
}

#[test]
fn test_ac6_missing_target_produces_empty_hypotheses() {
    // If a target is not in the graph, bfs_inbound returns empty (no inbound edges)
    // The engine itself won't error — the CLI layer does exit-1.
    // But we can test that a nonexistent target yields no hypotheses.
    let graph = build_graph_many_components();
    let sa = json!({});
    let sb = json!({});

    // "NonExistent" is not in the graph — bfs_inbound returns nothing
    let paths = mcp_hypothesis_engine::bfs_inbound(&graph, "NonExistent", 4);
    assert!(paths.is_empty(), "no paths from absent target");

    let result = mcp_hypothesis_engine::run_engine(&graph, "NonExistent", 0.0, &sa, &sb, 4, 8);
    assert!(result.hypotheses.is_empty(), "absent target yields no hypotheses");
}
