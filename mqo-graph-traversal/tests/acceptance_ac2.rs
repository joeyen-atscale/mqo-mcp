//! AC2: related_measures(graph, measure_name, depth) returns all measures
//! reachable within `depth` hops via any edge type, in ranked order by graph
//! distance. Returns an empty vec (not an error) when the start node is not found.

use mqo_graph_traversal::{build_graph, related_measures};

fn fixture_json() -> &'static str {
    r#"{
        "measures": [
            {"unique_name": "revenue",  "name": "Revenue"},
            {"unique_name": "cost",     "name": "Cost"},
            {"unique_name": "quantity", "name": "Quantity"}
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
             "formula_refs": [{"unique_name": "revenue"}, {"unique_name": "cost"}]},
            {"unique_name": "margin", "name": "Margin",
             "formula_refs": [{"unique_name": "profit"}]}
        ],
        "edges": [
            {"from": "revenue",  "to": "l_year", "kind": "aggregates_via"},
            {"from": "cost",     "to": "l_year", "kind": "aggregates_via"},
            {"from": "quantity", "to": "l_year", "kind": "aggregates_via"}
        ]
    }"#
}

#[test]
fn related_measures_depth_and_missing() {
    let graph = build_graph(fixture_json()).expect("fixture must be valid");

    // Depth 1 from "profit": undirected neighbors via any edge.
    // margin --DerivesFrom--> profit, so margin IS 1 hop from profit (undirected).
    // revenue and cost also DerivesFrom profit (profit --DerivesFrom--> revenue/cost).
    let depth1 = related_measures(&graph, "profit", 1);
    let names1: Vec<_> = depth1.iter().map(|r| r.unique_name.as_str()).collect();
    assert!(
        names1.contains(&"revenue"),
        "depth-1 should include revenue; got {names1:?}"
    );
    assert!(
        names1.contains(&"cost"),
        "depth-1 should include cost; got {names1:?}"
    );
    // margin has formula_refs=[profit], so edge is margin→profit (1 hop undirected from profit)
    assert!(
        names1.contains(&"margin"),
        "depth-1 should include margin (it is a direct undirected neighbor of profit); got {names1:?}"
    );

    // Depth 2 from "revenue": should find profit (1 hop), margin (2 hops via profit)
    let depth2 = related_measures(&graph, "revenue", 2);
    let names2: Vec<_> = depth2.iter().map(|r| r.unique_name.as_str()).collect();
    assert!(
        names2.contains(&"profit"),
        "depth-2 from revenue should include profit; got {names2:?}"
    );
    assert!(
        names2.contains(&"margin"),
        "depth-2 from revenue should include margin (revenue→profit→margin); got {names2:?}"
    );

    // Results should be in ascending distance order
    for window in depth2.windows(2) {
        assert!(
            window[0].distance <= window[1].distance,
            "results must be in ascending distance order"
        );
    }

    // Missing start node → empty vec, not an error
    let missing = related_measures(&graph, "does_not_exist", 5);
    assert!(
        missing.is_empty(),
        "unknown start node should return empty vec, not an error"
    );
}
