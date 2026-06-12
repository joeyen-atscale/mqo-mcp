// AC6: to_json produces JSON that round-trips through from_json to an
// equivalent graph (same nodes, same edges).

use std::collections::HashSet;

use mcp_concept_graph::{ConceptGraph, Edge, EdgeKind, Node, NodeKind};

fn build_test_graph() -> ConceptGraph {
    let mut g = ConceptGraph::new();
    let mut n1 = Node::new("m1", "Revenue", NodeKind::Measure);
    n1.model_name = "sales".into();
    n1.attributes.insert("folder".into(), serde_json::Value::String("Financial".into()));
    g.add_node(n1);

    let n2 = Node::new("hier_date", "Date Hierarchy", NodeKind::Hierarchy);
    g.add_node(n2);

    let n3 = Node::new("lvl_year", "Year", NodeKind::DimensionLevel);
    g.add_node(n3);

    g.add_edge(Edge::new("lvl_year", "hier_date", EdgeKind::LevelOf));
    g.add_edge(Edge::new("m1", "hier_date", EdgeKind::RelatedTo).with_weight(0.5));
    g
}

#[test]
fn test_roundtrip_node_count() {
    let original = build_test_graph();
    let json = original.to_json();
    let restored = ConceptGraph::from_json(&json);
    assert_eq!(original.nodes().len(), restored.nodes().len());
}

#[test]
fn test_roundtrip_node_ids() {
    let original = build_test_graph();
    let json = original.to_json();
    let restored = ConceptGraph::from_json(&json);

    let orig_ids: HashSet<String> = original.nodes().iter().map(|n| n.id.clone()).collect();
    let rest_ids: HashSet<String> = restored.nodes().iter().map(|n| n.id.clone()).collect();
    assert_eq!(orig_ids, rest_ids);
}

#[test]
fn test_roundtrip_edge_count() {
    let original = build_test_graph();
    let json = original.to_json();
    let restored = ConceptGraph::from_json(&json);
    assert_eq!(original.edges().len(), restored.edges().len());
}

#[test]
fn test_roundtrip_edge_kinds() {
    let original = build_test_graph();
    let json = original.to_json();
    let restored = ConceptGraph::from_json(&json);

    // from_json should preserve edge kinds
    let level_of = restored
        .edges()
        .into_iter()
        .filter(|e| e.kind == EdgeKind::LevelOf)
        .count();
    assert_eq!(level_of, 1);
}

#[test]
fn test_roundtrip_node_kinds() {
    let original = build_test_graph();
    let json = original.to_json();
    let restored = ConceptGraph::from_json(&json);

    assert_eq!(
        restored.nodes_by_kind(NodeKind::Measure).len(),
        original.nodes_by_kind(NodeKind::Measure).len()
    );
    assert_eq!(
        restored.nodes_by_kind(NodeKind::Hierarchy).len(),
        original.nodes_by_kind(NodeKind::Hierarchy).len()
    );
    assert_eq!(
        restored.nodes_by_kind(NodeKind::DimensionLevel).len(),
        original.nodes_by_kind(NodeKind::DimensionLevel).len()
    );
}

#[test]
fn test_roundtrip_edge_weights() {
    let original = build_test_graph();
    let json = original.to_json();
    let restored = ConceptGraph::from_json(&json);

    let rel_edge_orig = original
        .edges()
        .into_iter()
        .find(|e| e.kind == EdgeKind::RelatedTo)
        .unwrap();
    let rel_edge_rest = restored
        .edges()
        .into_iter()
        .find(|e| e.kind == EdgeKind::RelatedTo)
        .unwrap();
    assert!((rel_edge_orig.weight - rel_edge_rest.weight).abs() < 1e-6);
}

#[test]
fn test_roundtrip_attributes() {
    let original = build_test_graph();
    let json = original.to_json();
    let restored = ConceptGraph::from_json(&json);

    let orig_node = original.node("m1").unwrap();
    let rest_node = restored.node("m1").unwrap();
    assert_eq!(orig_node.model_name, rest_node.model_name);
    assert_eq!(orig_node.attributes.get("folder"), rest_node.attributes.get("folder"));
}

#[test]
fn test_roundtrip_from_describe_model() {
    use serde_json::json;
    let model = json!({
        "name": "rt_model",
        "measures": [
            { "unique_name": "rev", "name": "Revenue" },
            { "unique_name": "cost", "name": "Cost" }
        ],
        "dimensions": [
            {
                "unique_name": "dim_time",
                "name": "Time",
                "hierarchies": [
                    {
                        "unique_name": "hier_time",
                        "name": "Time Hierarchy",
                        "levels": [
                            { "unique_name": "lvl_yr", "name": "Year" },
                            { "unique_name": "lvl_mo", "name": "Month" }
                        ]
                    }
                ]
            }
        ]
    });

    let original = ConceptGraph::from_describe_model(&model).unwrap();
    let json = original.to_json();
    let restored = ConceptGraph::from_json(&json);

    assert_eq!(original.nodes().len(), restored.nodes().len());
    assert_eq!(original.edges().len(), restored.edges().len());
}
