// AC7 (SHOULD): from_describe_model on a 500-measure, 200-level, 100-calc
// model completes in under 200ms; k_hop_neighbors at k=3 on the same graph
// completes in under 50ms.

use std::time::Instant;

use mcp_concept_graph::ConceptGraph;
use serde_json::{json, Value};

fn build_large_model() -> Value {
    let measures: Vec<Value> = (0..500)
        .map(|i| {
            json!({
                "unique_name": format!("measure_{}", i),
                "name": format!("Measure {}", i),
                "folder": format!("Folder{}", i % 10)
            })
        })
        .collect();

    // 4 hierarchies × 50 levels each = 200 levels
    let dimensions: Vec<Value> = (0..4)
        .map(|d| {
            let levels: Vec<Value> = (0..50)
                .map(|l| {
                    json!({
                        "unique_name": format!("dim{}_level_{}", d, l),
                        "name": format!("Dim {} Level {}", d, l)
                    })
                })
                .collect();
            json!({
                "unique_name": format!("dim_{}", d),
                "name": format!("Dimension {}", d),
                "hierarchies": [{
                    "unique_name": format!("hier_{}", d),
                    "name": format!("Hierarchy {}", d),
                    "levels": levels
                }]
            })
        })
        .collect();

    let calcs: Vec<Value> = (0..100)
        .map(|i| {
            json!({
                "unique_name": format!("calc_{}", i),
                "name": format!("Calc {}", i),
                "expression": format!(
                    "[Measures].[Measure {}] + [Measures].[Measure {}]",
                    i * 2 % 500,
                    (i * 2 + 1) % 500
                )
            })
        })
        .collect();

    json!({
        "name": "large_model",
        "measures": measures,
        "dimensions": dimensions,
        "calculated_members": calcs
    })
}

#[test]
fn test_from_describe_model_under_200ms() {
    let model = build_large_model();
    let start = Instant::now();
    let g = ConceptGraph::from_describe_model(&model).unwrap();
    let elapsed = start.elapsed();

    // Sanity-check the result.
    assert!(
        g.nodes().len() >= 500 + 200 + 100,
        "expected >= 800 nodes, got {}",
        g.nodes().len()
    );

    assert!(
        elapsed.as_millis() < 200,
        "from_describe_model took {}ms (>200ms budget)",
        elapsed.as_millis()
    );
}

#[test]
fn test_k_hop_at_k3_under_50ms() {
    let model = build_large_model();
    let g = ConceptGraph::from_describe_model(&model).unwrap();

    // Pick the first calc node as our root.
    let root = "calc_0";

    let start = Instant::now();
    let _ = g.k_hop_neighbors(root, 3);
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_millis() < 50,
        "k_hop_neighbors(k=3) took {}ms (>50ms budget)",
        elapsed.as_millis()
    );
}
