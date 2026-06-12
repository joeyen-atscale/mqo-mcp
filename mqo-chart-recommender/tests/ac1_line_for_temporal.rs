//! AC1 (MUST): 1 measure + 1 temporal dim → mark = Line,
//! x = temporal field, y = measure field.

use mqo_chart_recommender::{recommend, Mark};
use serde_json::json;

#[test]
fn ac1_line_for_temporal() {
    let profile = json!({
        "schema": "result-profile.v1",
        "columns": [
            {"name": "order_date", "role": "dimension", "is_temporal": true,  "cardinality": 365},
            {"name": "revenue",    "role": "measure",   "is_temporal": false, "cardinality": null}
        ]
    });

    let rec = recommend(&profile).expect("recommend should succeed");

    assert_eq!(rec.mark, Mark::Line, "mark must be Line for temporal dim");

    let x = rec.encoding.x.as_ref().expect("x channel must be set");
    assert_eq!(x.field, "order_date", "x field must be the temporal dimension");
    assert_eq!(x.data_type, "temporal", "x data_type must be 'temporal'");

    let y = rec.encoding.y.as_ref().expect("y channel must be set");
    assert_eq!(y.field, "revenue", "y field must be the measure");
    assert_eq!(y.data_type, "quantitative", "y data_type must be 'quantitative'");
}
