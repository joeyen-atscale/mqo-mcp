//! AC5: semi_additive and is_calc flags lifted from catalog.

use mqo_result_profiler::profile;

#[test]
fn ac5_semi_additive_and_is_calc() {
    let response = serde_json::json!({
        "rows": [
            {"balance": 1000.0, "margin_pct": 0.15, "year": 2021}
        ],
        "bound": {
            "measures": ["balance", "margin_pct"],
            "dimensions": ["year"]
        }
    });
    let catalog = serde_json::json!({
        "columns": [
            {
                "unique_name": "balance",
                "label": "Balance",
                "kind": "measure",
                "semi_additive": {
                    "trigger_hierarchies": ["time.calendar"]
                }
            },
            {
                "unique_name": "margin_pct",
                "label": "Margin %",
                "kind": "measure",
                "is_calc": true
            },
            {
                "unique_name": "year",
                "label": "Year",
                "kind": "dimension",
                "hierarchy": "time.calendar"
            }
        ]
    });

    let p = profile(&response, &catalog).expect("profile should succeed");

    let balance = p.columns.iter().find(|c| c.name == "balance").expect("balance");
    assert!(balance.semi_additive, "balance should be flagged semi_additive");
    assert!(!balance.is_calc, "balance should not be flagged is_calc");

    let margin = p.columns.iter().find(|c| c.name == "margin_pct").expect("margin_pct");
    assert!(margin.is_calc, "margin_pct should be flagged is_calc");
    assert!(!margin.semi_additive, "margin_pct should not be flagged semi_additive");

    let yr = p.columns.iter().find(|c| c.name == "year").expect("year");
    assert!(!yr.is_calc);
    assert!(!yr.semi_additive);
}

#[test]
fn ac5_no_flags_when_catalog_absent() {
    // When catalog has no entry for a column, flags default to false.
    let response = serde_json::json!({
        "rows": [{"revenue": 100.0}],
        "bound": {"measures": ["revenue"], "dimensions": []}
    });
    let catalog = serde_json::json!({"columns": []});
    let p = profile(&response, &catalog).unwrap();
    let rev = p.columns.iter().find(|c| c.name == "revenue").unwrap();
    assert!(!rev.is_calc);
    assert!(!rev.semi_additive);
}
