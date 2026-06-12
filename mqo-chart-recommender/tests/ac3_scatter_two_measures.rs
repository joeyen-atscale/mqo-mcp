//! AC3 (MUST): 2 measures, 0 dims → mark = Point (scatter),
//! x and y are the two measures.

use mqo_chart_recommender::{recommend, Mark};
use serde_json::json;

#[test]
fn ac3_scatter_two_measures() {
    let profile = json!({
        "schema": "result-profile.v1",
        "columns": [
            {"name": "revenue", "role": "measure", "is_temporal": false, "cardinality": null},
            {"name": "cost",    "role": "measure", "is_temporal": false, "cardinality": null}
        ]
    });

    let rec = recommend(&profile).expect("recommend should succeed");

    assert_eq!(rec.mark, Mark::Point, "mark must be Point for 2-measure scatter");

    let x = rec.encoding.x.as_ref().expect("x channel must be set");
    assert_eq!(x.data_type, "quantitative");

    let y = rec.encoding.y.as_ref().expect("y channel must be set");
    assert_eq!(y.data_type, "quantitative");

    // Both channels must be the two measure fields.
    let fields: Vec<&str> = vec![x.field.as_str(), y.field.as_str()];
    assert!(
        fields.contains(&"revenue") && fields.contains(&"cost"),
        "x and y must be the two measure fields, got {fields:?}"
    );
}
