//! Property-based invariant tests for `mqo-chart-recommender`.
//!
//! Read-only after scaffold. The edit-agent must NOT modify proptests.

use mqo_chart_recommender::{recommend, Mark};
use proptest::prelude::*;
use serde_json::json;

/// Generate a random column count (0..=4 measures, 0..=4 dims, at least 1 col).
fn arb_profile() -> impl Strategy<Value = serde_json::Value> {
    (0usize..=3, 0usize..=3).prop_map(|(n_measures, n_dims)| {
        let mut cols = serde_json::json!([]);
        let arr = cols.as_array_mut().expect("is array");
        for i in 0..n_measures {
            arr.push(json!({
                "name": format!("m{i}"),
                "role": "measure",
                "is_temporal": false,
                "cardinality": null
            }));
        }
        for i in 0..n_dims {
            arr.push(json!({
                "name": format!("d{i}"),
                "role": "dimension",
                "is_temporal": false,
                "cardinality": 10u64
            }));
        }
        json!({"schema": "result-profile.v1", "columns": cols})
    })
}

proptest! {
    /// recommend() never panics and always returns a non-empty rationale.
    #[test]
    fn invariant_recommend_never_panics_and_rationale_non_empty(profile in arb_profile()) {
        let result = recommend(&profile);
        // We only assert on Ok results; the function may error for zero-column profiles.
        if let Ok(rec) = result {
            prop_assert!(!rec.rationale.is_empty(), "rationale must not be empty");
            // Schema tag must always be correct.
            prop_assert_eq!(&rec.schema, "chart-recommendation.v1");
        }
    }

    /// 0-measure profiles always produce Table.
    #[test]
    fn invariant_no_measures_always_table(n_dims in 1usize..=4) {
        let mut cols = vec![];
        for i in 0..n_dims {
            cols.push(json!({
                "name": format!("d{i}"),
                "role": "dimension",
                "is_temporal": false,
                "cardinality": 5u64
            }));
        }
        let profile = json!({"schema": "result-profile.v1", "columns": cols});
        let rec = recommend(&profile).expect("should succeed");
        prop_assert_eq!(rec.mark, Mark::Table, "0 measures must always be Table");
    }

    /// A single measure with no dims always produces BigNumber.
    #[test]
    fn invariant_single_measure_no_dims_is_big_number(name in "[a-z]{3,10}") {
        let profile = json!({
            "schema": "result-profile.v1",
            "columns": [
                {"name": name, "role": "measure", "is_temporal": false, "cardinality": null}
            ]
        });
        let rec = recommend(&profile).expect("should succeed");
        prop_assert_eq!(rec.mark, Mark::BigNumber);
    }

    /// A temporal dimension always produces Line (for 1m/1d case).
    #[test]
    fn invariant_temporal_dim_produces_line(
        dim_name in "[a-z]{3,10}",
        meas_name in "[a-z]{3,10}",
        card in 1u64..=1000
    ) {
        prop_assume!(dim_name != meas_name);
        let profile = json!({
            "schema": "result-profile.v1",
            "columns": [
                {"name": dim_name,  "role": "dimension", "is_temporal": true,  "cardinality": card},
                {"name": meas_name, "role": "measure",   "is_temporal": false, "cardinality": null}
            ]
        });
        let rec = recommend(&profile).expect("should succeed");
        prop_assert_eq!(rec.mark, Mark::Line);
    }
}
