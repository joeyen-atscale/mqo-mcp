//! RULE 2 (PRD-mqo-validator-semi-additive-guard): reject an additive sum of a
//! semi-additive measure over a time dimension.
//!
//! DORMANT on the live fixture (the recorded snapshot nulls `semi_additive`);
//! these tests use a synthetic `semi_additive == true` measure per PRD §7/OQ-1.
//!
//! Acceptance criteria:
//! - AC-1: EXPLICIT sum of a semi-additive measure over Sold Calendar Month →
//!   reject. A None/default agg is the model's semi-additive function → safe.
//! - AC-2: same measure with NO time dimension → no rejection.
//! - AC-3: a non-semi-additive measure summed over months → no rejection.
//! - AC-4: semi_additive null → no rejection (dormant).

use mqo_param_validator::{
    validate, BoundMqoInput, CatalogMeasure, CatalogSnapshot, MqoDimensionRef, MqoMeasureRef,
    RejectReason,
};

fn catalog() -> CatalogSnapshot {
    CatalogSnapshot {
        measures: vec![
            // Synthetic semi-additive balance with a declared `last` policy.
            CatalogMeasure {
                unique_name: "Inventory On Hand".to_string(),
                label: Some("Inventory On Hand".to_string()),
                semi_additive: Some(true),
                semi_additive_agg: Some("last".to_string()),
                ..Default::default()
            },
            // Semi-additive with NO declared policy (suggestion → avg-over-period).
            CatalogMeasure {
                unique_name: "Account Balance".to_string(),
                label: Some("Account Balance".to_string()),
                semi_additive: Some(true),
                ..Default::default()
            },
            // Ordinary additive measure.
            CatalogMeasure {
                unique_name: "Total Store Sales".to_string(),
                label: Some("Total Store Sales".to_string()),
                ..Default::default()
            },
            // semi_additive null (the live-fixture shape).
            CatalogMeasure {
                unique_name: "Units Sold".to_string(),
                label: Some("Units Sold".to_string()),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

fn sa_rejections(r: &[mqo_param_validator::ParamRejection]) -> Vec<&mqo_param_validator::ParamRejection> {
    r.iter()
        .filter(|x| matches!(x.reason, RejectReason::SemiAdditiveSum { .. }))
        .collect()
}

fn time_dim() -> MqoDimensionRef {
    MqoDimensionRef {
        unique_name: "Sold Calendar".to_string(),
        level: Some("Sold Calendar Month".to_string()),
        hierarchy: Some("Sold Calendar".to_string()),
        role_qualifier: None,
        ..Default::default()
    }
}

fn non_time_dim() -> MqoDimensionRef {
    MqoDimensionRef {
        unique_name: "Product".to_string(),
        level: Some("Product Class".to_string()),
        hierarchy: Some("Product".to_string()),
        role_qualifier: None,
        ..Default::default()
    }
}

// --- AC-1: sum semi-additive over time → reject + suggested agg ------------

#[test]
fn ac1_default_agg_semi_additive_over_month_not_rejected() {
    // A None/default aggregation on a semi-additive measure is NOT a misuse: the
    // engine applies the measure's semi-additive function (last-non-empty) under
    // the default, so "balance by period" is legitimate. Rejecting it would
    // false-positive every inventory-on-hand-by-month query. Only an EXPLICIT
    // additive override (sum) is rejected — see the explicit-sum test below.
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Inventory On Hand".to_string(),
            aggregation: None, // default → engine applies semi-additive agg → safe
        }],
        dimensions: vec![time_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    let sa = sa_rejections(&result);
    assert_eq!(
        sa.len(),
        0,
        "default-agg semi-additive over time must NOT be rejected: {result:?}"
    );
}

#[test]
fn ac1_explicit_sum_semi_additive_with_policy_suggests_last() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Inventory On Hand".to_string(),
            aggregation: Some("sum".to_string()), // explicit override → misuse
        }],
        dimensions: vec![time_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    let sa = sa_rejections(&result);
    assert_eq!(sa.len(), 1, "expected one semi-additive rejection: {result:?}");
    match &sa[0].reason {
        RejectReason::SemiAdditiveSum {
            measure,
            time_dimension,
            suggested_agg,
        } => {
            assert_eq!(measure, "Inventory On Hand");
            assert_eq!(time_dimension, "Sold Calendar Month");
            assert_eq!(suggested_agg, "last", "uses declared policy");
        }
        other => panic!("wrong reason: {other:?}"),
    }
}

#[test]
fn ac1_explicit_sum_agg_rejected_no_policy_suggests_avg() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Account Balance".to_string(),
            aggregation: Some("sum".to_string()),
        }],
        dimensions: vec![time_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    let sa = sa_rejections(&result);
    assert_eq!(sa.len(), 1, "{result:?}");
    if let RejectReason::SemiAdditiveSum { suggested_agg, .. } = &sa[0].reason {
        assert_eq!(suggested_agg, "average over period");
    } else {
        panic!();
    }
}

// --- AC-2: no time dimension → no rejection --------------------------------

#[test]
fn ac2_no_time_dimension_not_rejected() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Inventory On Hand".to_string(),
            aggregation: None,
        }],
        dimensions: vec![non_time_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert!(sa_rejections(&result).is_empty(), "no time dim → no rejection: {result:?}");
}

#[test]
fn ac2_no_dimensions_at_all_not_rejected() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Inventory On Hand".to_string(),
            aggregation: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert!(sa_rejections(&result).is_empty(), "{result:?}");
}

// --- AC-3: non-semi-additive measure summed over time → no rejection -------

#[test]
fn ac3_additive_measure_over_time_not_rejected() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Total Store Sales".to_string(),
            aggregation: Some("sum".to_string()),
        }],
        dimensions: vec![time_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert!(sa_rejections(&result).is_empty(), "additive over time is fine: {result:?}");
}

// --- AC-4: semi_additive null → no rejection (dormant) ---------------------

#[test]
fn ac4_semi_additive_null_not_rejected() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Units Sold".to_string(),
            aggregation: Some("sum".to_string()),
        }],
        dimensions: vec![time_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert!(sa_rejections(&result).is_empty(), "null flag → dormant: {result:?}");
}

// --- Edge: already using a non-additive aggregation → no rejection ---------

#[test]
fn already_last_aggregation_not_rejected() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Inventory On Hand".to_string(),
            aggregation: Some("last".to_string()),
        }],
        dimensions: vec![time_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert!(sa_rejections(&result).is_empty(), "non-additive agg → no rejection: {result:?}");
}
