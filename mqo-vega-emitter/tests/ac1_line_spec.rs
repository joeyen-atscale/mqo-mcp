//! AC1 — line spec shape.
//!
//! A Line recommendation (x=`year` temporal, y=`revenue` quantitative) over
//! matching rows emits a spec with `$schema` pointing at VL5, `mark:"line"`,
//! `encoding.x.field=="year"` with type `temporal`, and
//! `encoding.y.field=="revenue"` with type `quantitative`.

use mqo_vega_emitter::{emit, VL5_SCHEMA};
use serde_json::json;

#[test]
fn ac1_line_spec() {
    let rec = json!({
        "mark": "Line",
        "encoding": {
            "x": { "field": "year", "data_type": "temporal" },
            "y": { "field": "revenue", "data_type": "quantitative" }
        }
    });
    let rows = vec![json!({"year": "2023", "revenue": 100})];

    let spec = emit(&rec, &rows).expect("emit must succeed");

    assert_eq!(spec["$schema"], VL5_SCHEMA, "$schema must point at VL5");
    assert_eq!(spec["mark"], "line", "mark must be 'line'");
    assert_eq!(
        spec["encoding"]["x"]["field"], "year",
        "x.field must be 'year'"
    );
    assert_eq!(
        spec["encoding"]["x"]["type"], "temporal",
        "x.type must be 'temporal'"
    );
    assert_eq!(
        spec["encoding"]["y"]["field"], "revenue",
        "y.field must be 'revenue'"
    );
    assert_eq!(
        spec["encoding"]["y"]["type"], "quantitative",
        "y.type must be 'quantitative'"
    );
}
