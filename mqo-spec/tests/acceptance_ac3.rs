//! AC3: validate() rejects empty measures, limit 0, and Range lo > hi —
//!       each with a distinct MqoError variant.

use mqo_spec::{Filter, Mqo, MqoError, MeasureRef, validate};

fn minimal() -> Mqo {
    Mqo {
        model: "test".to_string(),
        measures: vec![MeasureRef { unique_name: "m.val".to_string() }],
        dimensions: vec![],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: None,
        non_empty: false,
    }
}

#[test]
fn validate_rejects_empty_measures() {
    let mut mqo = minimal();
    mqo.measures.clear();
    let errs = validate(&mqo).expect_err("should fail with empty measures");
    assert!(
        errs.iter().any(|e| matches!(e, MqoError::EmptyMeasures)),
        "expected EmptyMeasures error, got: {errs:?}"
    );
}

#[test]
fn validate_rejects_limit_zero() {
    let mut mqo = minimal();
    mqo.limit = Some(0);
    let errs = validate(&mqo).expect_err("should fail with limit=0");
    assert!(
        errs.iter().any(|e| matches!(e, MqoError::LimitZero)),
        "expected LimitZero error, got: {errs:?}"
    );
}

#[test]
fn validate_rejects_range_lo_gt_hi() {
    let mut mqo = minimal();
    mqo.filters.push(Filter::Range {
        level: "year".to_string(),
        lo: 2025.0,
        hi: 2020.0,
    });
    let errs = validate(&mqo).expect_err("should fail with lo > hi");
    assert!(
        errs.iter().any(|e| matches!(e, MqoError::RangeLoGtHi { lo, hi } if *lo > *hi)),
        "expected RangeLoGtHi error, got: {errs:?}"
    );
}

#[test]
fn validate_empty_measures_is_distinct_from_limit_zero() {
    // Both can co-exist — they are distinct error variants.
    let mqo = Mqo {
        model: "test".to_string(),
        measures: vec![],
        dimensions: vec![],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: Some(0),
        non_empty: false,
    };
    let errs = validate(&mqo).expect_err("should collect multiple errors");
    let has_empty = errs.iter().any(|e| matches!(e, MqoError::EmptyMeasures));
    let has_limit = errs.iter().any(|e| matches!(e, MqoError::LimitZero));
    assert!(has_empty, "EmptyMeasures missing from: {errs:?}");
    assert!(has_limit, "LimitZero missing from: {errs:?}");
}

#[test]
fn validate_accepts_limit_one() {
    let mut mqo = minimal();
    mqo.limit = Some(1);
    assert!(validate(&mqo).is_ok());
}

#[test]
fn validate_accepts_range_lo_eq_hi() {
    let mut mqo = minimal();
    mqo.filters.push(Filter::Range {
        level: "year".to_string(),
        lo: 2024.0,
        hi: 2024.0,
    });
    assert!(validate(&mqo).is_ok());
}
