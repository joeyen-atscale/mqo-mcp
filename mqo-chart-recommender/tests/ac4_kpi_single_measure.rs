//! AC4 (MUST): 1 measure, 0 dims → mark = BigNumber.

use mqo_chart_recommender::{recommend, Mark};
use serde_json::json;

#[test]
fn ac4_kpi_single_measure() {
    let profile = json!({
        "schema": "result-profile.v1",
        "columns": [
            {"name": "total_revenue", "role": "measure", "is_temporal": false, "cardinality": null}
        ]
    });

    let rec = recommend(&profile).expect("recommend should succeed");

    assert_eq!(rec.mark, Mark::BigNumber, "mark must be BigNumber for a single KPI measure");

    // No x channel expected for a KPI card.
    assert!(rec.encoding.x.is_none(), "x channel must be None for BigNumber");
}
