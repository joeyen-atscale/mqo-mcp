//! AC6 (MUST): 0-measure profile → mark = Table with rationale explaining
//! nothing is quantitative.

use mqo_chart_recommender::{recommend, Mark};
use serde_json::json;

#[test]
fn ac6_table_no_measures() {
    let profile = json!({
        "schema": "result-profile.v1",
        "columns": [
            {"name": "region",   "role": "dimension", "is_temporal": false, "cardinality": 5},
            {"name": "category", "role": "dimension", "is_temporal": false, "cardinality": 10}
        ]
    });

    let rec = recommend(&profile).expect("recommend should succeed");

    assert_eq!(rec.mark, Mark::Table, "mark must be Table when there are no measures");

    let rationale_lower = rec.rationale.to_lowercase();
    assert!(
        rationale_lower.contains("quantitative")
            || rationale_lower.contains("measure")
            || rationale_lower.contains("no"),
        "rationale must explain why Table was chosen; got: `{}`",
        rec.rationale
    );
}
