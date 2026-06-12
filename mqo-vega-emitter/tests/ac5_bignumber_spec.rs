//! AC5 — BigNumber spec.
//!
//! A BigNumber recommendation emits a `"text"`-mark spec with the measure
//! field in `encoding.text`.

use mqo_vega_emitter::emit;
use serde_json::json;

#[test]
fn ac5_bignumber_spec() {
    let rec = json!({
        "mark": "BigNumber",
        "encoding": {
            "y": { "field": "total_revenue", "data_type": "quantitative" }
        }
    });
    let rows = vec![
        json!({"total_revenue": 1_000_000}),
        json!({"total_revenue": 2_000_000}),
    ];

    let spec = emit(&rec, &rows).expect("emit must succeed");

    assert_eq!(spec["mark"], "text", "BigNumber must emit mark:'text'");
    assert_eq!(
        spec["encoding"]["text"]["field"], "total_revenue",
        "encoding.text.field must be the measure field"
    );
    // Should have aggregate:sum for an additive quantitative measure.
    assert_eq!(
        spec["encoding"]["text"]["aggregate"], "sum",
        "additive BigNumber must have aggregate:sum"
    );
}

#[test]
fn ac5_bignumber_no_x_y() {
    let rec = json!({
        "mark": "BigNumber",
        "encoding": {
            "y": { "field": "kpi_value", "data_type": "quantitative" }
        }
    });
    let rows = vec![json!({"kpi_value": 42})];

    let spec = emit(&rec, &rows).expect("emit must succeed");

    // BigNumber spec must NOT have x or y encoding — only text.
    assert!(
        spec["encoding"].get("x").is_none() || spec["encoding"]["x"].is_null(),
        "BigNumber spec must not have x encoding"
    );
    assert!(
        spec["encoding"].get("y").is_none() || spec["encoding"]["y"].is_null(),
        "BigNumber spec must not have y encoding"
    );
    assert!(
        spec["encoding"]["text"].is_object(),
        "encoding.text must be present"
    );
}
