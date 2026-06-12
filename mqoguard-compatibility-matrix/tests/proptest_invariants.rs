//! Property-based invariant tests for mqoguard-compatibility-matrix.
//!
//! Read-only after scaffold — the edit-agent must NOT modify these tests.
//! If a test is wrong, write `agent/intent_card_amendment_request.json`.

use mqoguard_compatibility_matrix::{
    build_matrix, is_symmetric, EnrichedCatalog, EnrichedColumn, MatrixConfig,
};
use proptest::prelude::*;
use std::collections::BTreeSet;

fn arb_groups() -> impl Strategy<Value = BTreeSet<String>> {
    proptest::collection::btree_set(
        prop_oneof![
            Just("sales".to_string()),
            Just("inventory".to_string()),
            Just("returns".to_string()),
        ],
        0..=2,
    )
}

proptest! {
    /// Invariant: the matrix is always symmetric regardless of input shape.
    #[test]
    fn prop_always_symmetric(
        n_measures in 1usize..=5,
        n_levels in 1usize..=8,
        measure_groups in proptest::collection::vec(arb_groups(), 1..=5),
        level_groups in proptest::collection::vec(arb_groups(), 1..=8),
        hier_indices in proptest::collection::vec(0usize..=2, 1..=8),
    ) {
        let hier_names = ["HierA", "HierB", "HierC"];

        let measures: Vec<EnrichedColumn> = (0..n_measures)
            .map(|i| {
                let groups = measure_groups.get(i % measure_groups.len()).cloned().unwrap_or_default();
                EnrichedColumn {
                    unique_name: format!("measure_{i}"),
                    label: format!("Measure {i}"),
                    kind: "measure".into(),
                    is_calc: false,
                    hierarchy: None,
                    level: None,
                    column_group: groups,
                }
            })
            .collect();

        let levels: Vec<EnrichedColumn> = (0..n_levels)
            .map(|i| {
                let groups = level_groups.get(i % level_groups.len()).cloned().unwrap_or_default();
                let hier_idx = hier_indices.get(i % hier_indices.len()).copied().unwrap_or(0).min(2);
                let hier = hier_names.get(hier_idx).copied().unwrap_or("HierA").to_string();
                EnrichedColumn {
                    unique_name: format!("level_{i}"),
                    label: format!("Level {i}"),
                    kind: "dimension".into(),
                    is_calc: false,
                    hierarchy: Some(hier),
                    level: Some(format!("L{i}")),
                    column_group: groups,
                }
            })
            .collect();

        let mut columns = measures;
        columns.extend(levels);
        let catalog = EnrichedCatalog { model: "prop_model".into(), columns };
        let matrix = build_matrix(&catalog, &MatrixConfig::default());
        prop_assert!(is_symmetric(&matrix), "symmetry violated");
    }

    /// Invariant: every measure that appears in the matrix was in the catalog.
    #[test]
    fn prop_no_phantom_measures(
        n in 1usize..=4,
        groups in proptest::collection::vec(arb_groups(), 1..=4),
    ) {
        let columns: Vec<EnrichedColumn> = (0..n)
            .map(|i| {
                let g = groups.get(i % groups.len()).cloned().unwrap_or_default();
                EnrichedColumn {
                    unique_name: format!("m_{i}"),
                    label: format!("M {i}"),
                    kind: "measure".into(),
                    is_calc: false,
                    hierarchy: None,
                    level: None,
                    column_group: g,
                }
            })
            .collect();
        // No dimension levels → no hierarchies → empty matrix with note (fail-safe)
        // or empty compatible sets
        let catalog = EnrichedCatalog { model: "prop".into(), columns };
        let matrix = build_matrix(&catalog, &MatrixConfig::default());
        for key in matrix.measures.keys() {
            // Every key in the matrix must have come from the catalog
            prop_assert!(
                catalog.columns.iter().any(|c| &c.unique_name == key),
                "phantom measure {key} appeared in matrix"
            );
        }
    }
}
