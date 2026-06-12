// AC2: a calc whose expression contains [Measures].[Total Store Sales]
// has a DerivesFrom edge to the Total Store Sales node.

use mcp_concept_graph::{ConceptGraph, EdgeKind};
use serde_json::json;

#[test]
fn test_derives_from_measure() {
    let model = json!({
        "name": "store_model",
        "measures": [
            { "unique_name": "total_store_sales", "name": "Total Store Sales" },
            { "unique_name": "total_store_cost",  "name": "Total Store Cost" }
        ],
        "calculated_members": [
            {
                "unique_name": "calc_margin",
                "name": "Store Margin",
                "expression": "[Measures].[Total Store Sales] - [Measures].[Total Store Cost]"
            }
        ]
    });

    let g = ConceptGraph::from_describe_model(&model).unwrap();

    let derives_edges: Vec<_> = g
        .edges()
        .into_iter()
        .filter(|e| e.kind == EdgeKind::DerivesFrom && e.from == "calc_margin")
        .collect();

    assert_eq!(derives_edges.len(), 2, "expected 2 DerivesFrom edges, got {}", derives_edges.len());

    let targets: Vec<&str> = derives_edges.iter().map(|e| e.to.as_str()).collect();
    assert!(
        targets.contains(&"total_store_sales"),
        "missing DerivesFrom edge to total_store_sales; targets = {:?}",
        targets
    );
    assert!(
        targets.contains(&"total_store_cost"),
        "missing DerivesFrom edge to total_store_cost; targets = {:?}",
        targets
    );
}

#[test]
fn test_derives_from_single_ref() {
    let model = json!({
        "name": "m",
        "measures": [
            { "unique_name": "total_store_sales", "name": "Total Store Sales" }
        ],
        "calculated_members": [
            {
                "unique_name": "calc_tax",
                "name": "Store Tax",
                "expression": "[Measures].[Total Store Sales] * 0.08"
            }
        ]
    });

    let g = ConceptGraph::from_describe_model(&model).unwrap();
    let derives: Vec<_> = g
        .edges()
        .into_iter()
        .filter(|e| e.kind == EdgeKind::DerivesFrom)
        .collect();
    assert_eq!(derives.len(), 1);
    assert_eq!(derives[0].from, "calc_tax");
    assert_eq!(derives[0].to, "total_store_sales");
}
