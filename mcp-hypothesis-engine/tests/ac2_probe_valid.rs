// AC2: Every emitted hypothesis carries a structurally valid probe_mqo
// (non-empty measures, each with a unique_name).

use mcp_concept_graph::{ConceptGraph, Edge, EdgeKind, Node, NodeKind};
use serde_json::json;

fn build_graph() -> ConceptGraph {
    let mut g = ConceptGraph::new();
    for name in &["Target", "CompA", "CompB", "CompC"] {
        let mut n = Node::new(*name, *name, NodeKind::Measure);
        n.model_name = "test".into();
        g.add_node(n);
    }
    g.add_edge(Edge::new("Target", "CompA", EdgeKind::DerivesFrom));
    g.add_edge(Edge::new("Target", "CompB", EdgeKind::AggregatesVia));
    g.add_edge(Edge::new("CompA", "CompC", EdgeKind::FiltersBy));
    g
}

#[test]
fn test_ac2_all_probes_valid() {
    let graph = build_graph();
    let sa = json!({});
    let sb = json!({});

    let result = mcp_hypothesis_engine::run_engine(&graph, "Target", -0.05, &sa, &sb, 4, 10);

    assert!(!result.hypotheses.is_empty());
    for h in &result.hypotheses {
        let measures = h.probe_mqo
            .get("measures")
            .and_then(|v| v.as_array())
            .expect("probe_mqo must have measures array");
        assert!(!measures.is_empty(), "measures must be non-empty for hypothesis {}", h.rank);
        for m in measures {
            assert!(
                m.get("unique_name").and_then(|v| v.as_str()).is_some(),
                "each measure must have unique_name for hypothesis {}", h.rank
            );
        }
        // dimensions and filters must be present
        assert!(h.probe_mqo.get("dimensions").is_some());
        assert!(h.probe_mqo.get("filters").is_some());
    }
}
