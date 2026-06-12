//! AC5 (MUST): high-cardinality nominal dim (> 25) still returns mark = Bar,
//! but alternatives contains Table with a cardinality rationale.

use mqo_chart_recommender::{recommend, Mark};
use serde_json::json;

#[test]
fn ac5_high_cardinality_alt() {
    let profile = json!({
        "schema": "result-profile.v1",
        "columns": [
            {"name": "product_sku", "role": "dimension", "is_temporal": false, "cardinality": 500},
            {"name": "revenue",     "role": "measure",   "is_temporal": false, "cardinality": null}
        ]
    });

    let rec = recommend(&profile).expect("recommend should succeed");

    assert_eq!(rec.mark, Mark::Bar, "primary mark must still be Bar for high-cardinality nominal");

    let has_table_alt = rec
        .alternatives
        .iter()
        .any(|a| a.mark == Mark::Table);
    assert!(
        has_table_alt,
        "alternatives must contain Table for high-cardinality dim; got {:?}",
        rec.alternatives.iter().map(|a| &a.mark).collect::<Vec<_>>()
    );

    // Rationale or the Table alternative's reason must mention cardinality.
    let table_alt = rec
        .alternatives
        .iter()
        .find(|a| a.mark == Mark::Table)
        .expect("Table alternative must exist");

    let mentions_cardinality = table_alt.reason.to_lowercase().contains("cardinality")
        || table_alt.reason.contains("500")
        || rec.rationale.to_lowercase().contains("cardinality")
        || rec.rationale.contains("500");

    assert!(
        mentions_cardinality,
        "Table alternative reason or rationale must reference cardinality; reason=`{}`, rationale=`{}`",
        table_alt.reason, rec.rationale
    );
}
