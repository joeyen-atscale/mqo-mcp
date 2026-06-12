//! AC2 — description is a single templated sentence matching the chosen mark.

use mqo_bi_asset_bundle::build_asset;
use serde_json::json;

#[test]
fn ac2_description_for_aggregating_line() {
    let response = json!({
        "rows": [
            {"revenue": 100.0, "year": "2021"},
            {"revenue": 200.0, "year": "2022"}
        ],
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
    assert_eq!(
        asset.description, "Sum of Revenue across Year.",
        "description for line/temporal should be 'Sum of Revenue across Year.', got '{}'",
        asset.description
    );
}

#[test]
fn ac2_description_for_bar_nominal() {
    let response = json!({
        "rows": [
            {"revenue": 100.0, "region": "East"},
            {"revenue": 200.0, "region": "West"}
        ],
        "bound": { "measures": ["revenue"], "dimensions": ["region"] }
    });
    let catalog = json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "region",  "label": "Region",  "kind": "dimension"}
        ]
    });
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    assert_eq!(
        asset.description, "Sum of Revenue across Region.",
        "description for bar/nominal should be 'Sum of Revenue across Region.', got '{}'",
        asset.description
    );
}

#[test]
fn ac2_description_for_scatter_two_measures() {
    let response = json!({
        "rows": [
            {"revenue": 100.0, "cost": 50.0},
            {"revenue": 200.0, "cost": 80.0}
        ],
        "bound": { "measures": ["revenue", "cost"], "dimensions": [] }
    });
    let catalog = json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "cost",    "label": "Cost",    "kind": "measure"}
        ]
    });
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    assert_eq!(
        asset.description, "Revenue vs Cost.",
        "description for two-measure scatter should be 'Revenue vs Cost.', got '{}'",
        asset.description
    );
}
