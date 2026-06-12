// AC3: Output always contains top-level evidence_type: "structural" and the verbatim analysis_note.

use mcp_concept_graph::{ConceptGraph, Edge, EdgeKind, Node, NodeKind};
use serde_json::json;

fn build_graph() -> ConceptGraph {
    let mut g = ConceptGraph::new();
    let mut tss = Node::new("Metric", "Metric", NodeKind::Measure);
    tss.model_name = "test".into();
    let mut comp = Node::new("Component", "Component", NodeKind::Measure);
    comp.model_name = "test".into();
    g.add_node(tss);
    g.add_node(comp);
    g.add_edge(Edge::new("Metric", "Component", EdgeKind::DerivesFrom));
    g
}

#[test]
fn test_ac3_evidence_type_and_note() {
    let graph = build_graph();
    let sa = json!({});
    let sb = json!({});

    let result = mcp_hypothesis_engine::run_engine(&graph, "Metric", -0.1, &sa, &sb, 4, 8);

    assert_eq!(result.evidence_type, "structural");
    assert_eq!(
        result.analysis_note,
        "Hypotheses are structural derivation paths with probe queries. Statistical causation requires additional analysis."
    );
}

#[test]
fn test_ac3_serialized_json_has_fields() {
    let graph = build_graph();
    let sa = json!({});
    let sb = json!({});

    let result = mcp_hypothesis_engine::run_engine(&graph, "Metric", 0.05, &sa, &sb, 4, 8);
    let v: serde_json::Value = serde_json::to_value(&result).unwrap();

    assert_eq!(v["evidence_type"], "structural");
    assert_eq!(
        v["analysis_note"],
        "Hypotheses are structural derivation paths with probe queries. Statistical causation requires additional analysis."
    );
}
