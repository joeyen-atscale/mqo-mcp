// AC4: A component with no data in either summary yields structural_only,
// ranked below corroborated hypotheses.

use mcp_concept_graph::{ConceptGraph, Edge, EdgeKind, Node, NodeKind};
use mcp_hypothesis_engine::Corroboration;
use serde_json::json;

fn build_graph() -> ConceptGraph {
    let mut g = ConceptGraph::new();
    for name in &["Total Sales", "Known Component", "Unknown Component"] {
        let mut n = Node::new(*name, *name, NodeKind::Measure);
        n.model_name = "test".into();
        g.add_node(n);
    }
    g.add_edge(Edge::new("Total Sales", "Known Component", EdgeKind::DerivesFrom));
    g.add_edge(Edge::new("Total Sales", "Unknown Component", EdgeKind::DerivesFrom));
    g
}

#[test]
fn test_ac4_structural_only_ranked_below_corroborated() {
    let graph = build_graph();
    // Only Known Component has data in summaries
    let sa = json!({ "columns": { "Known Component": { "mean": 500.0 } } });
    let sb = json!({ "columns": { "Known Component": { "mean": 460.0 } } });

    let result = mcp_hypothesis_engine::run_engine(&graph, "Total Sales", -0.08, &sa, &sb, 4, 8);

    assert!(result.hypotheses.len() >= 2, "need at least 2 hypotheses");

    // Find corroborated and structural_only
    let corroborated_pos = result.hypotheses.iter()
        .position(|h| h.corroboration == Corroboration::Corroborated);
    let structural_pos = result.hypotheses.iter()
        .position(|h| h.corroboration == Corroboration::StructuralOnly);

    assert!(corroborated_pos.is_some(), "should have a corroborated hypothesis");
    assert!(structural_pos.is_some(), "should have a structural_only hypothesis");

    assert!(
        corroborated_pos.unwrap() < structural_pos.unwrap(),
        "corroborated must rank before structural_only"
    );
}

#[test]
fn test_ac4_structural_only_probe_still_valid() {
    let graph = build_graph();
    let sa = json!({});
    let sb = json!({});

    let result = mcp_hypothesis_engine::run_engine(&graph, "Total Sales", -0.08, &sa, &sb, 4, 8);

    for h in &result.hypotheses {
        assert_eq!(h.corroboration, Corroboration::StructuralOnly);
        // probe must still be valid
        let measures = h.probe_mqo.get("measures").and_then(|v| v.as_array()).unwrap();
        assert!(!measures.is_empty());
    }
}
