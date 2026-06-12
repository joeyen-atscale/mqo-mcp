//! AC6 — semi-additive no sum.
//!
//! A quantitative y-channel gets `aggregate:"sum"` by default, but a
//! `semi_additive` measure does NOT get `aggregate:"sum"`.

use mqo_vega_emitter::emit;
use serde_json::json;

#[test]
fn ac6_additive_gets_sum() {
    let rec = json!({
        "mark": "Bar",
        "encoding": {
            "x": { "field": "region", "data_type": "nominal" },
            "y": { "field": "sales", "data_type": "quantitative" }
        }
    });
    let rows = vec![json!({"region": "North", "sales": 100})];

    let spec = emit(&rec, &rows).expect("emit must succeed");

    assert_eq!(
        spec["encoding"]["y"]["aggregate"], "sum",
        "additive quantitative measure must have aggregate:sum"
    );
}

#[test]
fn ac6_semi_additive_no_sum() {
    let rec = json!({
        "mark": "Bar",
        "encoding": {
            "x": { "field": "date", "data_type": "temporal" },
            "y": { "field": "account_balance", "data_type": "quantitative", "semi_additive": true }
        }
    });
    let rows = vec![json!({"date": "2024-01-01", "account_balance": 5000})];

    let spec = emit(&rec, &rows).expect("emit must succeed");

    // semi_additive measure must NOT have aggregate:sum — summing a balance over time is wrong.
    assert!(
        spec["encoding"]["y"]["aggregate"].is_null()
            || spec["encoding"]["y"].get("aggregate").is_none(),
        "semi-additive measure must NOT have aggregate:sum, got: {:?}",
        spec["encoding"]["y"]["aggregate"]
    );
    assert_eq!(
        spec["encoding"]["y"]["field"], "account_balance",
        "field must still be present"
    );
    assert_eq!(
        spec["encoding"]["y"]["type"], "quantitative",
        "type must still be present"
    );
}

#[test]
fn ac6_semi_additive_bignumber_no_sum() {
    let rec = json!({
        "mark": "BigNumber",
        "encoding": {
            "y": { "field": "ending_balance", "data_type": "quantitative", "semi_additive": true }
        }
    });
    let rows = vec![json!({"ending_balance": 99_999})];

    let spec = emit(&rec, &rows).expect("emit must succeed");

    // semi_additive BigNumber must also not aggregate.
    assert!(
        spec["encoding"]["text"]["aggregate"].is_null()
            || spec["encoding"]["text"].get("aggregate").is_none(),
        "semi-additive BigNumber must not have aggregate:sum"
    );
}
