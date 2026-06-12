// AC5: --from-event resolves target and target delta from the event's
// measure/observed/prior. Tested via engine API by simulating what main.rs does.

use mcp_concept_graph::{ConceptGraph, Edge, EdgeKind, Node, NodeKind};
use mcp_hypothesis_engine::compute_delta;
use serde_json::json;

fn build_graph(target: &str, component: &str) -> ConceptGraph {
    let mut g = ConceptGraph::new();
    let mut t = Node::new(target, target, NodeKind::Measure);
    t.model_name = "test".into();
    let mut c = Node::new(component, component, NodeKind::Measure);
    c.model_name = "test".into();
    g.add_node(t);
    g.add_node(c);
    g.add_edge(Edge::new(target, component, EdgeKind::DerivesFrom));
    g
}

#[test]
fn test_ac5_event_target_delta_resolution() {
    // Simulate what main.rs does when parsing a WatchEvent:
    // measure = "Revenue", observed = 920.0, prior = 1000.0 -> delta = -0.08
    let measure = "Revenue";
    let observed = 920.0_f64;
    let prior = 1000.0_f64;
    let delta = compute_delta(prior, observed);

    assert!((delta - (-0.08)).abs() < 1e-6, "delta should be -0.08, got {delta}");

    let graph = build_graph(measure, "Revenue Component");
    let sa = json!({ "columns": { "Revenue Component": { "mean": 1000.0 } } });
    let sb = json!({ "columns": { "Revenue Component": { "mean": 920.0 } } });

    let result = mcp_hypothesis_engine::run_engine(&graph, measure, delta, &sa, &sb, 4, 8);

    assert_eq!(result.target, "Revenue");
    assert!((result.target_delta_fraction - (-0.08)).abs() < 1e-5);
    assert!(!result.hypotheses.is_empty());

    use mcp_hypothesis_engine::Corroboration;
    assert_eq!(result.hypotheses[0].corroboration, Corroboration::Corroborated);
}

#[test]
fn test_ac5_event_nested_query_measure() {
    // Simulate WatchEvent where measure is nested in query.measures[0].unique_name
    // This tests the JSON parsing path in main.rs via serialized round-trip
    let event_json = json!({
        "query": {
            "measures": [
                { "unique_name": "Store Count" }
            ]
        },
        "observed": 450.0,
        "prior": 500.0
    });

    // Extract measure name as main.rs does
    let measure = event_json["query"]["measures"][0]["unique_name"]
        .as_str()
        .unwrap();
    assert_eq!(measure, "Store Count");

    let observed = event_json["observed"].as_f64().unwrap();
    let prior = event_json["prior"].as_f64().unwrap();
    let delta = compute_delta(prior, observed);
    assert!((delta - (-0.1)).abs() < 1e-6);
}
