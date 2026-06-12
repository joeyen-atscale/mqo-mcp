//! AC1 — revenue-by-year response yields title "Revenue by Year", a line vega_spec,
//! and a profile_summary listing Revenue (measure) and Year (dimension).

use mqo_bi_asset_bundle::build_asset;
use serde_json::json;

fn revenue_by_year_fixture() -> (serde_json::Value, serde_json::Value) {
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
fn ac1_title_is_revenue_by_year() {
    let (response, catalog) = revenue_by_year_fixture();
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed for revenue-by-year");
    assert_eq!(
        asset.title, "Revenue by Year",
        "title should be 'Revenue by Year', got '{}'",
        asset.title
    );
}

#[test]
fn ac1_vega_spec_mark_is_line() {
    let (response, catalog) = revenue_by_year_fixture();
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    let mark = asset.vega_spec.get("mark")
        .and_then(|v| v.as_str())
        .expect("vega_spec must have a 'mark' string field");
    assert_eq!(mark, "line", "mark should be 'line' for temporal dimension, got '{mark}'");
}

#[test]
fn ac1_profile_summary_lists_revenue_measure() {
    let (response, catalog) = revenue_by_year_fixture();
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    assert!(
        asset.profile_summary.measures.contains(&"Revenue".to_owned()),
        "profile_summary.measures should contain 'Revenue', got {:?}",
        asset.profile_summary.measures
    );
}

#[test]
fn ac1_profile_summary_lists_year_dimension() {
    let (response, catalog) = revenue_by_year_fixture();
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    assert!(
        asset.profile_summary.dimensions.contains(&"Year".to_owned()),
        "profile_summary.dimensions should contain 'Year', got {:?}",
        asset.profile_summary.dimensions
    );
}
