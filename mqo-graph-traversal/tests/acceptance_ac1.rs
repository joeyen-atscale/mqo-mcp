//! AC1: Given a describe_model JSON fixture, build_graph() returns a ConceptGraph
//! with correct node count and correct edge count. Panics on malformed JSON.

use mqo_graph_traversal::build_graph;

/// Fixture: 2 measures, 1 hierarchy, 1 level, 1 calc = 5 nodes.
/// Edges: 2 explicit aggregates_via + 1 RelatedTo (hierarchy→level) + 2 DerivesFrom = 5.
fn fixture_json() -> &'static str {
    r#"{
        "measures": [
            {"unique_name": "revenue", "name": "Revenue"},
            {"unique_name": "cost",    "name": "Cost"}
        ],
        "dimensions": [
            {"unique_name": "d_date", "name": "Date", "hierarchies": [
                {"unique_name": "h_cal", "name": "Calendar", "levels": [
                    {"unique_name": "l_year", "name": "Year"}
                ]}
            ]}
        ],
        "calculated_members": [
            {"unique_name": "profit", "name": "Profit",
             "formula_refs": [{"unique_name": "revenue"}, {"unique_name": "cost"}]}
        ],
        "edges": [
            {"from": "revenue", "to": "l_year", "kind": "aggregates_via"},
            {"from": "cost",    "to": "l_year", "kind": "aggregates_via"}
        ]
    }"#
}

#[test]
fn build_graph_node_edge_count() {
    let graph = build_graph(fixture_json()).expect("fixture JSON must be valid");

    // 2 measures + 1 hierarchy + 1 level + 1 calc = 5 nodes
    assert_eq!(
        graph.node_count(),
        5,
        "expected 5 nodes (2 measures + 1 hierarchy + 1 level + 1 calc)"
    );

    // 2 explicit aggregates_via + 1 RelatedTo (h_cal→l_year) + 2 DerivesFrom (profit→revenue, profit→cost) = 5 edges
    assert_eq!(
        graph.edge_count(),
        5,
        "expected 5 edges (2 aggregates_via + 1 related_to + 2 derives_from)"
    );
}

#[test]
#[should_panic]
fn build_graph_panics_on_empty_json() {
    // AC1 specifies: "Panics on malformed JSON."
    // build_graph returns Err on invalid JSON; unwrap() produces the panic.
    build_graph("").unwrap();
}

#[test]
#[should_panic]
fn build_graph_panics_on_invalid_json() {
    build_graph("not json at all {{{{").unwrap();
}
