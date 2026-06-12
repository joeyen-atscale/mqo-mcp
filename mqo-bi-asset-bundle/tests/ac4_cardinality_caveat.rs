//! AC4 — a high-cardinality nominal axis (> 25 distinct values) produces a clutter caveat.

use mqo_bi_asset_bundle::build_asset;
use serde_json::{Value, json};

/// Build a response with N distinct nominal categories.
fn make_high_card_response(n: usize) -> (Value, Value) {
    let rows: Vec<Value> = (0..n)
        .map(|i| json!({"revenue": i as f64 * 10.0, "product": format!("Product-{i:03}")}))
        .collect();
    let response = json!({
        "rows": rows,
        "bound": { "measures": ["revenue"], "dimensions": ["product"] }
    });
    let catalog = json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "product", "label": "Product", "kind": "dimension"}
        ]
    });
    (response, catalog)
}

#[test]
fn ac4_high_cardinality_produces_clutter_caveat() {
    let (response, catalog) = make_high_card_response(30); // 30 > 25
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    let has_clutter_caveat = asset.caveats.iter().any(|c| {
        c.contains("Product") && (c.contains("categories") || c.contains("cluttered"))
    });
    assert!(
        has_clutter_caveat,
        "caveats should include a clutter warning for 30 categories, got {:?}",
        asset.caveats
    );
}

#[test]
fn ac4_low_cardinality_no_clutter_caveat() {
    let (response, catalog) = make_high_card_response(5); // 5 <= 25
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    let has_clutter_caveat = asset.caveats.iter().any(|c| {
        c.contains("cluttered") || c.contains("categories")
    });
    assert!(
        !has_clutter_caveat,
        "clutter caveat should NOT fire for only 5 categories, got {:?}",
        asset.caveats
    );
}

#[test]
fn ac4_exactly_25_no_clutter_caveat() {
    let (response, catalog) = make_high_card_response(25); // == 25, not > 25
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    let has_clutter_caveat = asset.caveats.iter().any(|c| {
        c.contains("cluttered") || c.contains("categories")
    });
    assert!(
        !has_clutter_caveat,
        "clutter caveat should NOT fire for exactly 25 categories (threshold is > 25), got {:?}",
        asset.caveats
    );
}
