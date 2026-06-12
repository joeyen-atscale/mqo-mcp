//! AC4: measure_range = (min, max) over non-null rows; null_rate reflects nulls.

use mqo_result_profiler::profile;

#[test]
fn ac4_measure_range_and_null_rate() {
    let response = serde_json::json!({
        "rows": [
            {"revenue": 100.0, "region": "EMEA"},
            {"revenue": null,  "region": "AMER"},
            {"revenue": 300.0, "region": "APAC"},
            {"revenue": 50.0,  "region": "LATAM"}
        ],
        "bound": {
            "measures": ["revenue"],
            "dimensions": ["region"]
        }
    });
    let catalog = serde_json::json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "region",  "label": "Region",  "kind": "dimension"}
        ]
    });

    let p = profile(&response, &catalog).expect("profile should succeed");
    let rev = p.columns.iter().find(|c| c.name == "revenue").expect("revenue");

    // measure_range should be (50.0, 300.0) — only non-null values
    let (min, max) = rev.measure_range.expect("measure_range should be Some");
    assert!((min - 50.0).abs() < 1e-9, "min should be 50.0, got {min}");
    assert!((max - 300.0).abs() < 1e-9, "max should be 300.0, got {max}");

    // null_rate = 1/4 = 0.25
    assert!(
        (rev.null_rate - 0.25).abs() < 1e-9,
        "null_rate should be 0.25, got {}",
        rev.null_rate
    );
}

#[test]
fn ac4_dimension_has_no_measure_range() {
    let response = serde_json::json!({
        "rows": [{"revenue": 100.0, "region": "EMEA"}],
        "bound": {"measures": ["revenue"], "dimensions": ["region"]}
    });
    let catalog = serde_json::json!({"columns": [
        {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
        {"unique_name": "region",  "label": "Region",  "kind": "dimension"}
    ]});
    let p = profile(&response, &catalog).unwrap();
    let reg = p.columns.iter().find(|c| c.name == "region").unwrap();
    assert!(reg.measure_range.is_none(), "dimension should have no measure_range");
}
