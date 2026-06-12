//! AC2 — inline data verbatim.
//!
//! `data.values` equals the input rows verbatim (inline embedding, no
//! mutation or reordering of row objects).

use mqo_vega_emitter::emit;
use serde_json::json;

#[test]
fn ac2_inline_data() {
    let rec = json!({
        "mark": "Bar",
        "encoding": {
            "x": { "field": "category", "data_type": "nominal" },
            "y": { "field": "amount", "data_type": "quantitative" }
        }
    });
    let rows = vec![
        json!({"category": "A", "amount": 10}),
        json!({"category": "B", "amount": 20}),
        json!({"category": "C", "amount": 30}),
    ];

    let spec = emit(&rec, &rows).expect("emit must succeed");

    let data_values = spec["data"]["values"]
        .as_array()
        .expect("data.values must be an array");

    assert_eq!(data_values.len(), rows.len(), "row count must match");
    for (i, (got, expected)) in data_values.iter().zip(rows.iter()).enumerate() {
        assert_eq!(got, expected, "row {i} must be verbatim");
    }
}
