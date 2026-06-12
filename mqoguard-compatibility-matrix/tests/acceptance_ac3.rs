//! AC3: For every (measure, hierarchy) pair in the output, the relation is
//! symmetric — measure lists hierarchy iff hierarchy lists measure.
//! Property-based test via proptest.

use mqoguard_compatibility_matrix::{
    build_matrix, is_symmetric, EnrichedCatalog, EnrichedColumn, MatrixConfig,
};
use proptest::prelude::*;
use std::collections::BTreeSet;

/// Generate an arbitrary set of column-group tags from a small alphabet.
fn column_groups() -> impl Strategy<Value = BTreeSet<String>> {
    proptest::collection::btree_set(
        prop_oneof![
            Just("sales".to_string()),
            Just("inventory".to_string()),
            Just("returns".to_string()),
            Just("catalog".to_string()),
        ],
        0..=3,
    )
}

/// Generate an arbitrary `EnrichedColumn`.
fn arb_column(
    kind: &'static str,
    index: usize,
    hierarchy: Option<String>,
) -> impl Strategy<Value = EnrichedColumn> {
    column_groups().prop_map(move |cg| EnrichedColumn {
        unique_name: format!("{kind}_{index}"),
        label: format!("{kind} {index}"),
        kind: kind.to_string(),
        is_calc: false,
        hierarchy: hierarchy.clone(),
        level: hierarchy.as_ref().map(|h| format!("{h}_level_{index}")),
        column_group: cg,
    })
}

proptest! {
    #[test]
    fn acceptance_ac3_symmetry_always_holds(
        measures in proptest::collection::vec(
            (0usize..10).prop_flat_map(|i| arb_column("measure", i, None)),
            1..=5,
        ),
        levels in proptest::collection::vec(
            (0usize..15, prop_oneof![
                Just("HierA".to_string()),
                Just("HierB".to_string()),
                Just("HierC".to_string()),
            ]).prop_flat_map(|(i, hier)| arb_column("dimension", i, Some(hier))),
            1..=8,
        ),
    ) {
        let mut columns = measures;
        columns.extend(levels);
        let catalog = EnrichedCatalog { model: "prop_test".into(), columns };
        let matrix = build_matrix(&catalog, &MatrixConfig::default());
        prop_assert!(
            is_symmetric(&matrix),
            "symmetry violated for catalog: {:?}", catalog
        );
    }
}
