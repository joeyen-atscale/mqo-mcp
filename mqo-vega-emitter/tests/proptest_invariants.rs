//! Property-test invariants for mqo-vega-emitter.
//!
//! READ-ONLY after scaffold. These invariants must hold for any valid input.

use mqo_vega_emitter::{emit, VL5_SCHEMA};
use proptest::prelude::*;
use serde_json::{json, Value};

proptest! {
    /// For any mark in the supported set, a spec with matching rows always
    /// emits a $schema pointing at VL5.
    #[test]
    fn prop_always_vl5_schema(
        mark in prop_oneof![
            Just("Line"),
            Just("Bar"),
            Just("Point"),
            Just("Area"),
            Just("Rect"),
        ],
        field_value in "[a-z]{3,8}",
    ) {
        let rec = json!({
            "mark": mark,
            "encoding": {
                "x": { "field": "dim", "data_type": "nominal" }
            }
        });
        let rows = vec![json!({"dim": field_value})];
        let spec = emit(&rec, &rows).expect("emit must succeed with valid input");
        prop_assert_eq!(spec["$schema"].as_str().unwrap_or(""), VL5_SCHEMA);
    }

    /// data.values is always verbatim — no mutation.
    #[test]
    fn prop_data_values_verbatim(
        n in 0usize..10usize,
        val in 0i64..1000i64,
    ) {
        let rows: Vec<Value> = (0..n).map(|i| json!({"x": i as i64, "metric": val + i as i64})).collect();
        let rec = json!({
            "mark": "Line",
            "encoding": {
                "x": { "field": "x", "data_type": "quantitative" },
                "y": { "field": "metric", "data_type": "quantitative" }
            }
        });
        let spec = emit(&rec, &rows).expect("emit must succeed");
        let got = spec["data"]["values"].as_array().expect("data.values must be array");
        prop_assert_eq!(got.len(), rows.len());
        for (got_row, exp_row) in got.iter().zip(rows.iter()) {
            prop_assert_eq!(got_row, exp_row);
        }
    }
}
