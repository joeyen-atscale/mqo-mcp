//! AC3: causal_paths(graph, target_measure) returns ranked Vec<CausalPath> where
//! each path carries the ordered list of edges and nodes from a root column/calc
//! to the target measure, with evidence_type: Structural on every result. At
//! least one path must be returned for any measure that has derivation edges.

use mqo_graph_traversal::{build_graph, causal_paths, EdgeKind, EvidenceType};

fn fixture_json() -> &'static str {
    r#"{
        "measures": [
            {"unique_name": "revenue", "name": "Revenue"},
            {"unique_name": "cost",    "name": "Cost"}
        ],
        "dimensions": [],
        "calculated_members": [
            {"unique_name": "profit", "name": "Profit",
             "formula_refs": [{"unique_name": "revenue"}, {"unique_name": "cost"}]},
            {"unique_name": "margin", "name": "Margin",
             "formula_refs": [{"unique_name": "profit"}]}
        ]
    }"#
}

#[test]
fn causal_paths_structural_evidence() {
    let graph = build_graph(fixture_json()).expect("fixture must be valid");

    // profit has DerivesFrom edges pointing to revenue and cost
    let paths_profit = causal_paths(&graph, "profit");
    assert!(
        !paths_profit.is_empty(),
        "profit has derivation edges; at least one causal path must be returned"
    );

    // Every path must have evidence_type = Structural
    for path in &paths_profit {
        assert_eq!(
            path.evidence_type,
            EvidenceType::Structural,
            "all causal paths must carry evidence_type: Structural"
        );
    }

    // Every path step must use a causal edge kind (DerivesFrom or FiltersBy)
    for path in &paths_profit {
        assert!(
            !path.steps.is_empty(),
            "each path must have at least one step"
        );
        for step in &path.steps {
            assert!(
                step.edge_kind == EdgeKind::DerivesFrom || step.edge_kind == EdgeKind::FiltersBy,
                "causal path steps must use DerivesFrom or FiltersBy edges; got {:?}",
                step.edge_kind
            );
        }
    }

    // Paths should be ranked ascending by length
    for window in paths_profit.windows(2) {
        assert!(
            window[0].length <= window[1].length,
            "causal paths must be sorted ascending by length"
        );
    }

    // margin derives_from profit which derives_from revenue and cost
    // → there should be at least one path to margin
    let paths_margin = causal_paths(&graph, "margin");
    assert!(
        !paths_margin.is_empty(),
        "margin has derivation edges; at least one causal path must be returned"
    );

    // A measure with no derivation edges should return empty vec
    let paths_revenue = causal_paths(&graph, "revenue");
    assert!(
        paths_revenue.is_empty(),
        "revenue has no DerivesFrom edges pointing to it; should return empty vec"
    );

    // Unknown measure → empty vec
    let paths_missing = causal_paths(&graph, "nonexistent");
    assert!(paths_missing.is_empty(), "unknown target should return empty vec");
}
