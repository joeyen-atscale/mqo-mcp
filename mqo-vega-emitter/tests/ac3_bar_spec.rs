//! AC3 — bar spec.
//!
//! A Bar recommendation emits `mark:"bar"` with the nominal field on the
//! x channel.

use mqo_vega_emitter::emit;
use serde_json::json;

#[test]
fn ac3_bar_spec() {
    let rec = json!({
        "mark": "Bar",
        "encoding": {
            "x": { "field": "product", "data_type": "nominal" },
            "y": { "field": "sales", "data_type": "quantitative" }
        }
    });
    let rows = vec![
        json!({"product": "Widget", "sales": 500}),
        json!({"product": "Gadget", "sales": 300}),
    ];

    let spec = emit(&rec, &rows).expect("emit must succeed");

    assert_eq!(spec["mark"], "bar", "mark must be 'bar'");
    assert_eq!(
        spec["encoding"]["x"]["field"], "product",
        "x.field must be 'product'"
    );
    assert_eq!(
        spec["encoding"]["x"]["type"], "nominal",
        "x.type must be 'nominal'"
    );
}
