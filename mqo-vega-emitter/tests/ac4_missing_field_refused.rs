//! AC4 — missing field refused.
//!
//! An encoding channel referencing a field absent from every row returns
//! `EmitError` (does not emit a broken spec).

use mqo_vega_emitter::{emit, EmitError};
use serde_json::json;

#[test]
fn ac4_missing_field_refused() {
    let rec = json!({
        "mark": "Line",
        "encoding": {
            "x": { "field": "year", "data_type": "temporal" },
            "y": { "field": "NONEXISTENT_FIELD", "data_type": "quantitative" }
        }
    });
    // The rows only have "year" and "revenue", not "NONEXISTENT_FIELD".
    let rows = vec![json!({"year": "2023", "revenue": 100})];

    let result = emit(&rec, &rows);

    assert!(result.is_err(), "emit must fail when a field is absent from all rows");
    match result {
        Err(EmitError::MissingField { field, .. }) => {
            assert_eq!(field, "NONEXISTENT_FIELD");
        }
        other => panic!("expected EmitError::MissingField, got: {other:?}"),
    }
}

#[test]
fn ac4_missing_field_x_channel() {
    let rec = json!({
        "mark": "Bar",
        "encoding": {
            "x": { "field": "ghost_dimension", "data_type": "nominal" },
            "y": { "field": "amount", "data_type": "quantitative" }
        }
    });
    let rows = vec![json!({"amount": 42})];

    let result = emit(&rec, &rows);

    assert!(result.is_err(), "emit must fail on missing x field");
    match result {
        Err(EmitError::MissingField { channel, field }) => {
            assert_eq!(field, "ghost_dimension");
            assert_eq!(channel, "x");
        }
        other => panic!("expected EmitError::MissingField, got: {other:?}"),
    }
}
