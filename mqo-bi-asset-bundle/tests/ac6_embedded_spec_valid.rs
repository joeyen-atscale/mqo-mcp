//! AC6 — the vega_spec embedded in the bundle is a valid VL5 spec
//! (has $schema, mark, encoding).

use mqo_bi_asset_bundle::build_asset;
use serde_json::json;

#[test]
fn ac6_vega_spec_has_schema_field() {
    let response = json!({
        "rows": [{"revenue": 100.0, "year": "2021"}],
        "bound": { "measures": ["revenue"], "dimensions": ["year"] }
    });
    let catalog = json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "year",    "label": "Year",    "kind": "dimension",
             "hierarchy": "time.calendar"}
        ]
    });
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    let schema_val = asset.vega_spec.get("$schema")
        .expect("vega_spec must contain '$schema'");
    let schema_str = schema_val.as_str().expect("$schema must be a string");
    assert!(
        schema_str.contains("vega-lite"),
        "$schema should reference vega-lite, got '{schema_str}'"
    );
}

#[test]
fn ac6_vega_spec_has_mark_field() {
    let response = json!({
        "rows": [{"revenue": 100.0, "year": "2021"}],
        "bound": { "measures": ["revenue"], "dimensions": ["year"] }
    });
    let catalog = json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "year",    "label": "Year",    "kind": "dimension",
             "hierarchy": "time.calendar"}
        ]
    });
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    assert!(
        asset.vega_spec.get("mark").is_some(),
        "vega_spec must contain 'mark'"
    );
}

#[test]
fn ac6_vega_spec_has_encoding_field() {
    let response = json!({
        "rows": [{"revenue": 100.0, "year": "2021"}],
        "bound": { "measures": ["revenue"], "dimensions": ["year"] }
    });
    let catalog = json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "year",    "label": "Year",    "kind": "dimension",
             "hierarchy": "time.calendar"}
        ]
    });
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    assert!(
        asset.vega_spec.get("encoding").is_some(),
        "vega_spec must contain 'encoding'"
    );
}

#[test]
fn ac6_vega_spec_has_inline_data() {
    let response = json!({
        "rows": [{"revenue": 100.0, "year": "2021"}, {"revenue": 200.0, "year": "2022"}],
        "bound": { "measures": ["revenue"], "dimensions": ["year"] }
    });
    let catalog = json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "year",    "label": "Year",    "kind": "dimension",
             "hierarchy": "time.calendar"}
        ]
    });
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    let data_values = asset.vega_spec
        .get("data")
        .and_then(|d| d.get("values"))
        .and_then(|v| v.as_array())
        .expect("vega_spec must have data.values array");
    assert_eq!(
        data_values.len(), 2,
        "data.values should have 2 rows, got {}",
        data_values.len()
    );
}
