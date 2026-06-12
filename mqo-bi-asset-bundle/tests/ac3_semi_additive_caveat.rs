//! AC3 — a semi_additive measure over a temporal axis produces a caveat naming
//! the aggregation risk.

use mqo_bi_asset_bundle::build_asset;
use serde_json::json;

#[test]
fn ac3_semi_additive_over_temporal_produces_caveat() {
    let response = json!({
        "rows": [
            {"inventory": 500, "month": "2024-01"},
            {"inventory": 450, "month": "2024-02"},
            {"inventory": 600, "month": "2024-03"}
        ],
        "bound": { "measures": ["inventory"], "dimensions": ["month"] }
    });
    // semi_additive field present on the measure
    let catalog = json!({
        "columns": [
            {
                "unique_name": "inventory",
                "label": "Inventory",
                "kind": "measure",
                "semi_additive": { "type": "last_non_empty" }
            },
            {
                "unique_name": "month",
                "label": "Month",
                "kind": "dimension",
                "hierarchy": "time.calendar"
            }
        ]
    });
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    assert!(
        !asset.caveats.is_empty(),
        "caveats must be non-empty for a semi-additive measure over a temporal axis"
    );
    let has_semi_additive_caveat = asset.caveats.iter().any(|c| {
        c.contains("Inventory") && c.contains("semi-additive")
    });
    assert!(
        has_semi_additive_caveat,
        "caveats should include a semi-additive warning mentioning 'Inventory' and 'semi-additive', got {:?}",
        asset.caveats
    );
}

#[test]
fn ac3_semi_additive_over_nominal_no_caveat() {
    // semi_additive over a NON-temporal (nominal) dim — rule (a) does not fire
    let response = json!({
        "rows": [
            {"balance": 500.0, "region": "East"},
            {"balance": 450.0, "region": "West"}
        ],
        "bound": { "measures": ["balance"], "dimensions": ["region"] }
    });
    let catalog = json!({
        "columns": [
            {
                "unique_name": "balance",
                "label": "Balance",
                "kind": "measure",
                "semi_additive": { "type": "last_non_empty" }
            },
            {
                "unique_name": "region",
                "label": "Region",
                "kind": "dimension"
            }
        ]
    });
    let asset = build_asset(&response, &catalog).expect("build_asset must succeed");
    let has_semi_additive_caveat = asset.caveats.iter().any(|c| {
        c.contains("semi-additive")
    });
    assert!(
        !has_semi_additive_caveat,
        "semi-additive caveat should NOT fire over a non-temporal dimension, got {:?}",
        asset.caveats
    );
}
