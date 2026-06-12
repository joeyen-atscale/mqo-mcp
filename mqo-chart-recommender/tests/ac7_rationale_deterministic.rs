//! AC7 (MUST): every recommendation carries a non-empty rationale and
//! deterministic alternatives ordering — calling recommend() twice on the
//! same input must return identical output.

use mqo_chart_recommender::recommend;
use serde_json::json;

fn profiles() -> Vec<serde_json::Value> {
    vec![
        // 1 measure, 1 temporal dim
        json!({
            "schema": "result-profile.v1",
            "columns": [
                {"name": "date",    "role": "dimension", "is_temporal": true,  "cardinality": 365},
                {"name": "revenue", "role": "measure",   "is_temporal": false, "cardinality": null}
            ]
        }),
        // 1 measure, 1 nominal dim
        json!({
            "schema": "result-profile.v1",
            "columns": [
                {"name": "region",  "role": "dimension", "is_temporal": false, "cardinality": 5},
                {"name": "revenue", "role": "measure",   "is_temporal": false, "cardinality": null}
            ]
        }),
        // 1 measure, 0 dims (KPI)
        json!({
            "schema": "result-profile.v1",
            "columns": [
                {"name": "revenue", "role": "measure", "is_temporal": false, "cardinality": null}
            ]
        }),
        // 2 measures, 0 dims (scatter)
        json!({
            "schema": "result-profile.v1",
            "columns": [
                {"name": "revenue", "role": "measure", "is_temporal": false, "cardinality": null},
                {"name": "cost",    "role": "measure", "is_temporal": false, "cardinality": null}
            ]
        }),
        // 0 measures (table)
        json!({
            "schema": "result-profile.v1",
            "columns": [
                {"name": "region", "role": "dimension", "is_temporal": false, "cardinality": 5}
            ]
        }),
        // high cardinality
        json!({
            "schema": "result-profile.v1",
            "columns": [
                {"name": "sku",     "role": "dimension", "is_temporal": false, "cardinality": 500},
                {"name": "revenue", "role": "measure",   "is_temporal": false, "cardinality": null}
            ]
        }),
    ]
}

#[test]
fn ac7_rationale_non_empty() {
    for (i, profile) in profiles().iter().enumerate() {
        let rec = recommend(profile).unwrap_or_else(|e| panic!("profile[{i}] error: {e}"));
        assert!(
            !rec.rationale.is_empty(),
            "profile[{i}]: rationale must not be empty"
        );
    }
}

#[test]
fn ac7_deterministic_alternatives_ordering() {
    for (i, profile) in profiles().iter().enumerate() {
        let rec1 = recommend(profile).unwrap_or_else(|e| panic!("profile[{i}] first call: {e}"));
        let rec2 = recommend(profile).unwrap_or_else(|e| panic!("profile[{i}] second call: {e}"));

        // Marks must match.
        assert_eq!(
            rec1.mark, rec2.mark,
            "profile[{i}]: mark not deterministic"
        );

        // Alternatives must be identical in order and content.
        assert_eq!(
            rec1.alternatives.len(),
            rec2.alternatives.len(),
            "profile[{i}]: alternatives length differs"
        );
        for (j, (a1, a2)) in rec1.alternatives.iter().zip(rec2.alternatives.iter()).enumerate() {
            assert_eq!(
                a1.mark, a2.mark,
                "profile[{i}] alternatives[{j}]: mark differs"
            );
            assert_eq!(
                a1.reason, a2.reason,
                "profile[{i}] alternatives[{j}]: reason differs"
            );
        }
    }
}
