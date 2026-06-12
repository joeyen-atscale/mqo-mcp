//! Acceptance-test aggregator — one fn per AC for the run-metrics.sh counter.
//!
//! The full test logic lives in the per-AC files (`ac1_revenue_by_year.rs`,
//! `ac2_description.rs`, …). This monolithic file exists so that
//! `scripts/run-metrics.sh` can count MUST-AC totals and passing counts via the
//! `'^fn ac[0-9]+_'` and `'^test ac[0-9]+_[a-z0-9_]+ \.\.\. ok'` grep patterns.

use mqo_bi_asset_bundle::build_asset;
use serde_json::json;

fn revenue_by_year() -> (serde_json::Value, serde_json::Value) {
    let response = json!({
        "rows": [
            {"revenue": 100.0, "year": "2021"},
            {"revenue": 200.0, "year": "2022"},
            {"revenue": 150.0, "year": "2023"}
        ],
        "bound": { "measures": ["revenue"], "dimensions": ["year"] }
    });
    let catalog = json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "year", "label": "Year", "kind": "dimension",
             "hierarchy": "time.calendar"}
        ]
    });
    (response, catalog)
}

#[test]
fn ac1_revenue_by_year_title_and_line_spec() {
    let (response, catalog) = revenue_by_year();
    let asset = build_asset(&response, &catalog).expect("AC1: build_asset must succeed");
    assert_eq!(asset.title, "Revenue by Year");
    assert_eq!(asset.vega_spec.get("mark").and_then(|v| v.as_str()), Some("line"));
    assert!(asset.profile_summary.measures.contains(&"Revenue".to_owned()));
    assert!(asset.profile_summary.dimensions.contains(&"Year".to_owned()));
}

#[test]
fn ac2_description_is_templated_sentence() {
    let (response, catalog) = revenue_by_year();
    let asset = build_asset(&response, &catalog).expect("AC2: build_asset must succeed");
    assert_eq!(asset.description, "Sum of Revenue across Year.");
}

#[test]
fn ac3_semi_additive_caveat_fires() {
    let response = json!({
        "rows": [
            {"inventory": 500, "month": "2024-01"},
            {"inventory": 450, "month": "2024-02"}
        ],
        "bound": { "measures": ["inventory"], "dimensions": ["month"] }
    });
    let catalog = json!({
        "columns": [
            {"unique_name": "inventory", "label": "Inventory", "kind": "measure",
             "semi_additive": {"type": "last_non_empty"}},
            {"unique_name": "month", "label": "Month", "kind": "dimension",
             "hierarchy": "time.calendar"}
        ]
    });
    let asset = build_asset(&response, &catalog).expect("AC3: build_asset must succeed");
    let has_caveat = asset.caveats.iter().any(|c| c.contains("semi-additive"));
    assert!(has_caveat, "AC3: expected semi-additive caveat, got {:?}", asset.caveats);
}

#[test]
fn ac4_cardinality_caveat_fires() {
    let rows: Vec<serde_json::Value> = (0..30)
        .map(|i| json!({"revenue": i as f64, "product": format!("P-{i:03}")}))
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
    let asset = build_asset(&response, &catalog).expect("AC4: build_asset must succeed");
    let has_caveat = asset.caveats.iter().any(|c| c.contains("categories"));
    assert!(has_caveat, "AC4: expected cardinality caveat, got {:?}", asset.caveats);
}

#[test]
fn ac5_no_caveats_for_clean_case() {
    let response = json!({
        "rows": [
            {"revenue": 100.0, "region": "East"},
            {"revenue": 200.0, "region": "West"},
            {"revenue": 150.0, "region": "North"}
        ],
        "bound": { "measures": ["revenue"], "dimensions": ["region"] }
    });
    let catalog = json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "region",  "label": "Region",  "kind": "dimension"}
        ]
    });
    let asset = build_asset(&response, &catalog).expect("AC5: build_asset must succeed");
    assert!(asset.caveats.is_empty(), "AC5: expected empty caveats, got {:?}", asset.caveats);
}

#[test]
fn ac6_vega_spec_is_valid_vl5() {
    let (response, catalog) = revenue_by_year();
    let asset = build_asset(&response, &catalog).expect("AC6: build_asset must succeed");
    assert!(asset.vega_spec.get("$schema").is_some(), "AC6: vega_spec must have $schema");
    assert!(asset.vega_spec.get("mark").is_some(), "AC6: vega_spec must have mark");
    assert!(asset.vega_spec.get("encoding").is_some(), "AC6: vega_spec must have encoding");
}

#[test]
fn ac7_malformed_response_returns_error() {
    let bad = json!({"bad": "payload"});
    let catalog = json!({"columns": []});
    let result = build_asset(&bad, &catalog);
    assert!(result.is_err(), "AC7: malformed response must return Err");
}
