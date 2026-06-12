//! Property-based invariants for the enrichment pass.
//! READ-ONLY: the edit-agent must not modify this file.

use mqoguard_column_group_enrichment::{enrich, CatalogColumn, CatalogSnapshot, FactBindings};
use proptest::prelude::*;
use std::collections::BTreeMap;

fn arb_catalog_column() -> impl Strategy<Value = CatalogColumn> {
    (
        "[a-z_][a-z_0-9]{0,20}\\.[a-z_][a-z_0-9]{0,20}",
        prop::option::of(prop_oneof!["measure", "level"].prop_map(String::from)),
        prop::option::of("[a-z_][a-z_0-9]{0,15}".prop_map(String::from)),
        prop::option::of(any::<bool>()),
    )
        .prop_map(|(unique_name, kind, hierarchy, is_calc)| CatalogColumn {
            unique_name,
            kind,
            hierarchy,
            is_calc,
            ..Default::default()
        })
}

fn arb_catalog(max_cols: usize) -> impl Strategy<Value = CatalogSnapshot> {
    proptest::collection::vec(arb_catalog_column(), 0..=max_cols).prop_map(|columns| {
        CatalogSnapshot {
            columns,
            ..Default::default()
        }
    })
}

proptest! {
    /// Column count is preserved: output has exactly as many columns as input.
    #[test]
    fn prop_output_len_eq_input_len(catalog in arb_catalog(50)) {
        let bindings = FactBindings { measures: BTreeMap::new(), hierarchies: BTreeMap::new() };
        let enriched = enrich(&catalog, &bindings);
        prop_assert_eq!(enriched.columns.len(), catalog.columns.len());
    }

    /// Coverage totals are consistent: total = bound + unbound.
    #[test]
    fn prop_coverage_totals_consistent(catalog in arb_catalog(50)) {
        let bindings = FactBindings { measures: BTreeMap::new(), hierarchies: BTreeMap::new() };
        let enriched = enrich(&catalog, &bindings);
        prop_assert_eq!(
            enriched.coverage.total,
            enriched.coverage.bound + enriched.coverage.unbound_count
        );
    }

    /// unique_name is always preserved byte-identical.
    #[test]
    fn prop_unique_name_preserved(catalog in arb_catalog(20)) {
        let bindings = FactBindings { measures: BTreeMap::new(), hierarchies: BTreeMap::new() };
        let enriched = enrich(&catalog, &bindings);
        for (input, output) in catalog.columns.iter().zip(enriched.columns.iter()) {
            prop_assert_eq!(&input.unique_name, &output.unique_name);
        }
    }

    /// kind is always preserved.
    #[test]
    fn prop_kind_preserved(catalog in arb_catalog(20)) {
        let bindings = FactBindings { measures: BTreeMap::new(), hierarchies: BTreeMap::new() };
        let enriched = enrich(&catalog, &bindings);
        for (input, output) in catalog.columns.iter().zip(enriched.columns.iter()) {
            prop_assert_eq!(&input.kind, &output.kind);
        }
    }

    /// With empty bindings, all columns are unbound (coverage_pct == 0 or vacuously 1).
    #[test]
    fn prop_empty_bindings_all_unbound(catalog in arb_catalog(30)) {
        let bindings = FactBindings { measures: BTreeMap::new(), hierarchies: BTreeMap::new() };
        let enriched = enrich(&catalog, &bindings);
        if catalog.columns.is_empty() {
            // vacuous case: set explicitly to 1.0
            prop_assert!(
                (enriched.coverage.coverage_pct - 1.0_f64).abs() < f64::EPSILON,
                "empty catalog coverage_pct must be 1.0"
            );
        } else {
            prop_assert_eq!(enriched.coverage.unbound_count, catalog.columns.len());
            prop_assert!(
                enriched.coverage.coverage_pct.abs() < f64::EPSILON,
                "non-empty all-unbound coverage_pct must be 0.0"
            );
        }
    }

    /// schema is always "enriched-catalog.v1".
    #[test]
    fn prop_schema_field_is_v1(catalog in arb_catalog(10)) {
        let bindings = FactBindings { measures: BTreeMap::new(), hierarchies: BTreeMap::new() };
        let enriched = enrich(&catalog, &bindings);
        prop_assert_eq!(&enriched.schema, "enriched-catalog.v1");
    }
}
