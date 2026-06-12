//! AC2 (MUST): 1 measure + 1 nominal dim (low cardinality) → mark = Bar.

use mqo_chart_recommender::{recommend, Mark};
use serde_json::json;

#[test]
fn ac2_bar_for_nominal() {
    let profile = json!({
        "schema": "result-profile.v1",
        "columns": [
            {"name": "region",  "role": "dimension", "is_temporal": false, "cardinality": 5},
            {"name": "revenue", "role": "measure",   "is_temporal": false, "cardinality": null}
        ]
    });

    let rec = recommend(&profile).expect("recommend should succeed");

    assert_eq!(rec.mark, Mark::Bar, "mark must be Bar for nominal dim with low cardinality");

    let x = rec.encoding.x.as_ref().expect("x channel must be set");
    assert_eq!(x.field, "region", "x field must be the nominal dimension");
    assert_eq!(x.data_type, "nominal");

    let y = rec.encoding.y.as_ref().expect("y channel must be set");
    assert_eq!(y.field, "revenue");
    assert_eq!(y.data_type, "quantitative");
}
