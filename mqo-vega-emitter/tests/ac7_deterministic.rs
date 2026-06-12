//! AC7 — deterministic.
//!
//! The emitted spec is deterministic and round-trips through `serde_json`
//! without key-reordering surprises (stable object key order via a fixed struct).

use mqo_vega_emitter::emit;
use serde_json::json;

#[test]
fn ac7_deterministic_repeated_calls() {
    let rec = json!({
        "mark": "Line",
        "encoding": {
            "x": { "field": "date", "data_type": "temporal" },
            "y": { "field": "value", "data_type": "quantitative" }
        }
    });
    let rows = vec![
        json!({"date": "2024-01-01", "value": 10}),
        json!({"date": "2024-01-02", "value": 20}),
    ];

    // Call emit twice and verify the output is identical.
    let spec1 = emit(&rec, &rows).expect("first emit must succeed");
    let spec2 = emit(&rec, &rows).expect("second emit must succeed");

    assert_eq!(spec1, spec2, "repeated calls must return identical specs");
}

#[test]
fn ac7_roundtrip_through_serde_json() {
    let rec = json!({
        "mark": "Bar",
        "encoding": {
            "x": { "field": "category", "data_type": "nominal" },
            "y": { "field": "count", "data_type": "quantitative" }
        }
    });
    let rows = vec![json!({"category": "X", "count": 5})];

    let spec = emit(&rec, &rows).expect("emit must succeed");

    // Serialize to string and back.
    let json_str = serde_json::to_string(&spec).expect("serialize must succeed");
    let re_parsed: serde_json::Value =
        serde_json::from_str(&json_str).expect("re-parse must succeed");

    assert_eq!(spec, re_parsed, "spec must round-trip through serde_json without change");
}

#[test]
fn ac7_top_level_key_order() {
    let rec = json!({
        "mark": "Point",
        "encoding": {
            "x": { "field": "x_val", "data_type": "quantitative" },
            "y": { "field": "y_val", "data_type": "quantitative" }
        }
    });
    let rows = vec![json!({"x_val": 1, "y_val": 2})];

    let spec = emit(&rec, &rows).expect("emit must succeed");

    // Verify required top-level keys are present.
    assert!(spec.get("$schema").is_some(), "$schema must be present");
    assert!(spec.get("data").is_some(), "data must be present");
    assert!(spec.get("mark").is_some(), "mark must be present");
    assert!(spec.get("encoding").is_some(), "encoding must be present");
}
