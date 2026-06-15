//! Property-based invariant tests for mqo-mdx-compiler.
//!
//! Invariants under test:
//! 1. Any valid BoundMqoInput (≥1 measure, no semi-additive without trigger)
//!    compiles without error.
//! 2. Every compiled MDX contains `FROM [` (cube name always qualified).
//! 3. When dimensions are present, the output contains `NON EMPTY` and `ON ROWS`.
//! 4. The measure axis always contains `ON COLUMNS`.
//! 5. A semi-additive measure with empty trigger_hierarchies always errors.

use mqo_mdx_compiler::{
    compile, BoundDimensionInput, BoundMeasureInput, BoundMqoInput, MdxCompileError,
};
use mqo_spec::{MeasureRef, Mqo};
use proptest::prelude::*;

// ── Arbitrary strategies ────────────────────────────────────────────────────

fn arb_safe_string() -> impl Strategy<Value = String> {
    // Simple alphanumeric identifiers; avoids MDX metacharacters.
    "[a-z][a-z0-9_]{1,15}".prop_map(String::from)
}

fn arb_model_name() -> impl Strategy<Value = String> {
    // 1, 2, or 3 dot-separated segments.
    prop_oneof![
        arb_safe_string(),
        (arb_safe_string(), arb_safe_string())
            .prop_map(|(a, b)| format!("{a}.{b}")),
        (arb_safe_string(), arb_safe_string(), arb_safe_string())
            .prop_map(|(a, b, c)| format!("{a}.{b}.{c}")),
    ]
}

fn arb_plain_measure() -> impl Strategy<Value = BoundMeasureInput> {
    (arb_safe_string(), arb_safe_string()).prop_map(|(ns, name)| BoundMeasureInput {
        unique_name: format!("{ns}.{name}"),
        is_calc: false,
        semi_additive: false,
        required_dimension: None,
        trigger_hierarchies: vec![],
        mdx_dependency_hierarchies: vec![],
    })
}

fn arb_dimension() -> impl Strategy<Value = BoundDimensionInput> {
    (arb_safe_string(), arb_safe_string(), arb_safe_string()).prop_map(|(ns, hier, level)| {
        BoundDimensionInput {
            unique_name: format!("{ns}.{hier}.{level}"),
            hierarchy: format!("{ns}.{hier}"),
        }
    })
}

fn arb_bound_mqo_valid() -> impl Strategy<Value = BoundMqoInput> {
    (
        arb_model_name(),
        prop::collection::vec(arb_plain_measure(), 1..=4),
        prop::collection::vec(arb_dimension(), 0..=3),
    )
        .prop_map(|(model, measures, dimensions)| {
            let mqo_measures: Vec<MeasureRef> = measures
                .iter()
                .map(|m| MeasureRef {
                    unique_name: m.unique_name.clone(),
                })
                .collect();
            BoundMqoInput {
                mqo: Mqo {
                    model,
                    measures: mqo_measures,
                    dimensions: vec![],
                    filters: vec![],
                    time_intelligence: vec![],
                    order: None,
                    limit: None,
                    non_empty: true,
                    projection: false,
                },
                measures,
                dimensions,
                calc_group_members: vec![],
            }
        })
}

// ── Invariant tests ─────────────────────────────────────────────────────────

proptest! {
    /// INV-1/2/4: Any valid BoundMqo compiles; output always has `FROM [` and `ON COLUMNS`.
    #[test]
    fn prop_valid_input_always_compiles(bound in arb_bound_mqo_valid()) {
        let result = compile(&bound);
        prop_assert!(result.is_ok(), "compile failed: {:?}", result.err());
        let mdx = result.unwrap();
        prop_assert!(mdx.contains("FROM ["), "missing FROM clause: {mdx}");
        prop_assert!(mdx.contains("ON COLUMNS"), "missing ON COLUMNS: {mdx}");
    }

    /// INV-3: When dimensions are present, NON EMPTY and ON ROWS appear.
    #[test]
    fn prop_dims_produce_non_empty_row_axis(
        bound in arb_bound_mqo_valid().prop_filter(
            "need at least one dimension",
            |b| !b.dimensions.is_empty(),
        )
    ) {
        let mdx = compile(&bound).expect("compile must succeed");
        prop_assert!(mdx.contains("NON EMPTY"), "NON EMPTY missing when dims present: {mdx}");
        prop_assert!(mdx.contains("ON ROWS"), "ON ROWS missing when dims present: {mdx}");
    }

    /// INV-5: Semi-additive measure with empty trigger_hierarchies always errors.
    #[test]
    fn prop_semi_additive_no_trigger_always_errors(
        model in arb_model_name(),
        unique_name in "[a-z][a-z0-9_]{1,15}".prop_map(|s| format!("fin.{s}"))
    ) {
        let m = BoundMeasureInput {
            unique_name: unique_name.clone(),
            is_calc: false,
            semi_additive: true,
            required_dimension: None,
            trigger_hierarchies: vec![], // always empty → must error
            mdx_dependency_hierarchies: vec![],
        };
        let bound = BoundMqoInput {
            mqo: Mqo {
                model,
                measures: vec![MeasureRef { unique_name }],
                dimensions: vec![],
                filters: vec![],
                time_intelligence: vec![],
                order: None,
                limit: None,
                non_empty: true,
                projection: false,
            },
            measures: vec![m],
            dimensions: vec![],
            calc_group_members: vec![],
        };
        let err = compile(&bound).expect_err("must error for missing trigger");
        prop_assert!(
            matches!(err, MdxCompileError::SemiAdditiveMissingTrigger(_)),
            "wrong error variant: {err:?}"
        );
    }
}
