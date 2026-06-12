//! RULE 3 (PRD-mqo-validator-calc-aggregation-guard): reject sum/avg of a
//! ratio/percentage/average `is_calc` measure.
//!
//! Acceptance criteria:
//! - AC-1: sum an is_calc percentage measure → reject.
//! - AC-2: average an is_calc average measure → reject.
//! - AC-3: an additive `* Increase` calc aggregated → no rejection.
//! - AC-4: a non-calc base measure summed → no rejection.

use mqo_param_validator::{
    validate, BoundMqoInput, CalcKind, CatalogMeasure, CatalogSnapshot, MqoDimensionRef,
    MqoMeasureRef, RejectReason,
};

fn region_dim() -> MqoDimensionRef {
    MqoDimensionRef {
        unique_name: "Store".to_string(),
        level: Some("Store Region".to_string()),
        hierarchy: Some("Store".to_string()),
        role_qualifier: None,
        ..Default::default()
    }
}

fn catalog() -> CatalogSnapshot {
    let calc = |name: &str, is_calc: Option<bool>, kind: Option<CalcKind>| CatalogMeasure {
        unique_name: name.to_string(),
        label: Some(name.to_string()),
        is_calc,
        calc_kind: kind,
        ..Default::default()
    };
    CatalogSnapshot {
        measures: vec![
            // Percentage calc by name signal.
            calc("Gross Margin Pct", Some(true), None),
            // Average calc by name signal.
            calc("Catalog Sales Average Sales Price", Some(true), None),
            // Additive calc (* Increase) — must NOT be rejected.
            calc("Store Sales Increase", Some(true), None),
            // Calc flagged ratio explicitly via catalog calc_kind.
            calc("Conversion Index", Some(true), Some(CalcKind::Ratio)),
            // Calc flagged additive explicitly.
            calc("Bonus Pool", Some(true), Some(CalcKind::Additive)),
            // Non-calc base measure.
            CatalogMeasure {
                unique_name: "Total Store Sales".to_string(),
                label: Some("Total Store Sales".to_string()),
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}

fn calc_rejections(r: &[mqo_param_validator::ParamRejection]) -> Vec<&mqo_param_validator::ParamRejection> {
    r.iter()
        .filter(|x| matches!(x.reason, RejectReason::CalcMisaggregation { .. }))
        .collect()
}

// --- AC-1: sum a percentage calc → reject ----------------------------------

#[test]
fn ac1_sum_percentage_calc_rejected() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Gross Margin Pct".to_string(),
            aggregation: Some("sum".to_string()),
        }],
        dimensions: vec![region_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    let cr = calc_rejections(&result);
    assert_eq!(cr.len(), 1, "expected one calc-misaggregation rejection: {result:?}");
    if let RejectReason::CalcMisaggregation { measure, .. } = &cr[0].reason {
        assert_eq!(measure, "Gross Margin Pct");
    } else {
        panic!();
    }
}

#[test]
fn ac1_default_agg_percentage_calc_rejected() {
    // Default (None) aggregation is treated as additive (sum) → reject.
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Gross Margin Pct".to_string(),
            aggregation: None,
        }],
        dimensions: vec![region_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert_eq!(calc_rejections(&result).len(), 1, "{result:?}");
}

// --- AC-2: average an average calc → reject --------------------------------

#[test]
fn ac2_average_average_calc_rejected() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Catalog Sales Average Sales Price".to_string(),
            aggregation: Some("avg".to_string()),
        }],
        dimensions: vec![region_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert_eq!(calc_rejections(&result).len(), 1, "average-of-averages → reject: {result:?}");
}

#[test]
fn explicit_ratio_calc_kind_rejected() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Conversion Index".to_string(),
            aggregation: Some("sum".to_string()),
        }],
        dimensions: vec![region_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert_eq!(calc_rejections(&result).len(), 1, "{result:?}");
}

// --- AC-3: additive * Increase calc → no rejection -------------------------

#[test]
fn ac3_additive_increase_calc_not_rejected() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Store Sales Increase".to_string(),
            aggregation: Some("sum".to_string()),
        }],
        dimensions: vec![region_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert!(calc_rejections(&result).is_empty(), "additive calc must not be rejected: {result:?}");
}

#[test]
fn explicit_additive_calc_kind_not_rejected() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Bonus Pool".to_string(),
            aggregation: Some("sum".to_string()),
        }],
        dimensions: vec![region_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert!(calc_rejections(&result).is_empty(), "{result:?}");
}

// --- AC-4: non-calc base measure summed → no rejection ---------------------

#[test]
fn ac4_non_calc_base_measure_not_rejected() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Total Store Sales".to_string(),
            aggregation: Some("sum".to_string()),
        }],
        dimensions: vec![region_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert!(calc_rejections(&result).is_empty(), "non-calc → no rejection: {result:?}");
}

// --- Edge: ratio calc at own grain with a non-sum/avg agg → no rejection ---

#[test]
fn ratio_calc_last_aggregation_not_rejected() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Gross Margin Pct".to_string(),
            aggregation: Some("last".to_string()),
        }],
        dimensions: vec![region_dim()],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert!(calc_rejections(&result).is_empty(), "non sum/avg agg → no rejection: {result:?}");
}
