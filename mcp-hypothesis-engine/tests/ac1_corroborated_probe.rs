// AC1: Given Total Store Sales --DerivesFrom--> Store Sales Amount, both fell,
// top hypothesis has correct path, corroboration=corroborated, probe_mqo measure = Store Sales Amount.

use mcp_concept_graph::{ConceptGraph, Edge, EdgeKind, Node, NodeKind};
use serde_json::{json, Value};

fn build_graph() -> ConceptGraph {
    let mut g = ConceptGraph::new();
    let mut tss = Node::new("Total Store Sales", "Total Store Sales", NodeKind::Measure);
    tss.model_name = "test".into();
    let mut ssa = Node::new("Store Sales Amount", "Store Sales Amount", NodeKind::Measure);
    ssa.model_name = "test".into();
    g.add_node(tss);
    g.add_node(ssa);
    // Total Store Sales --DerivesFrom--> Store Sales Amount
    // In concept-graph terms: edge from "Total Store Sales" to "Store Sales Amount" with kind DerivesFrom
    // i.e. TSS derives FROM SSA
    g.add_edge(Edge::new("Total Store Sales", "Store Sales Amount", EdgeKind::DerivesFrom));
    g
}

fn summary(col: &str, mean: f64) -> Value {
    json!({ "columns": { col: { "mean": mean } } })
}

#[test]
fn test_ac1_corroborated_probe() {
    let graph = build_graph();
    // Both fell: TSS mean went from 1000 to 922 (-7.8%), SSA from 1000 to 925 (-7.5%)
    let sa = summary("Store Sales Amount", 1000.0);
    let sb = summary("Store Sales Amount", 925.0);

    let result = mcp_hypothesis_engine::run_engine(
        &graph,
        "Total Store Sales",
        -0.078,
        &sa,
        &sb,
        4,
        8,
    );

    assert_eq!(result.target, "Total Store Sales");
    assert!(!result.hypotheses.is_empty(), "must produce at least one hypothesis");

    let h = &result.hypotheses[0];
    assert_eq!(h.path, vec!["Total Store Sales", "Store Sales Amount"]);

    // corroboration = corroborated
    use mcp_hypothesis_engine::Corroboration;
    assert_eq!(h.corroboration, Corroboration::Corroborated, "should be corroborated");

    // probe_mqo has single measure with unique_name = Store Sales Amount
    let measures = h.probe_mqo.get("measures").and_then(|v| v.as_array()).unwrap();
    assert_eq!(measures.len(), 1);
    let uname = measures[0].get("unique_name").and_then(|v| v.as_str()).unwrap();
    assert_eq!(uname, "Store Sales Amount");
}
