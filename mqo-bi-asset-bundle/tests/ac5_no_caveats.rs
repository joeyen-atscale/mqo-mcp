//! AC5 — a clean case (additive measure, low-cardinality nominal dimension) produces caveats: [].

use mqo_bi_asset_bundle::build_asset;
use serde_json::json;

#[test]
fn ac5_clean_case_empty_caveats() {
    let response = json!({
        "rows": [
            {"revenue": 100.0, "region": "East"},
            {"revenue": 200.0, "region": "West"},
            {"revenue": 150.0, "region": "North"}
        ],
        "bound": { "measures": ["revenue"], "dimensions": ["region"] }
    });
    // additive measure (no semi_additive), low cardinality (3 <= 25), not is_calc
    let catalog = json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "region",  "label": "Region",  "kind": "dimension"}
        ]
    });
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    assert!(
        asset.caveats.is_empty(),
        "caveats should be empty for a clean additive/low-cardinality case, got {:?}",
        asset.caveats
    );
}
