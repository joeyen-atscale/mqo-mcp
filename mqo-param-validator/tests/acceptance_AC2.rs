//! AC2: An MQO naming a measure absent from the catalog returns one
//! ParamRejection with class: Measure, reason: Unmapped, and a non-empty
//! suggestions ranked by descending similarity (the lookalike_measure case).

use mqo_param_validator::{
    validate, BoundMqoInput, CatalogMeasure, CatalogSnapshot, FieldClass, MqoMeasureRef,
    RejectReason,
};

fn catalog_with_sales() -> CatalogSnapshot {
    CatalogSnapshot {
        measures: vec![
            CatalogMeasure { unique_name: "Sales Amount".to_string(), subject_area: None },
            CatalogMeasure { unique_name: "Total Revenue".to_string(), subject_area: None },
        ],
        ..Default::default()
    }
}

#[test]
fn ac2_lookalike_measure_rejected_with_suggestions() {
    // "Sales Amnt" is a near-miss for "Sales Amount"
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef { unique_name: "Sales Amnt".to_string() }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog_with_sales());
    assert_eq!(result.len(), 1, "Expected exactly one rejection: {result:?}");
    let rej = &result[0];
    assert_eq!(rej.class, FieldClass::Measure);
    assert_eq!(rej.reason, RejectReason::Unmapped);
    assert_eq!(rej.field, "Sales Amnt");
    assert!(
        !rej.suggestions.is_empty(),
        "Should have non-empty suggestions for a near-miss"
    );
    // Suggestions must be sorted descending by similarity
    for window in rej.suggestions.windows(2) {
        assert!(
            window[0].similarity >= window[1].similarity,
            "Suggestions must be sorted descending: {:?}",
            rej.suggestions
        );
    }
    // The top suggestion should be "Sales Amount"
    assert_eq!(rej.suggestions[0].name, "Sales Amount");
}

#[test]
fn ac2_completely_unknown_measure_rejected() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef { unique_name: "XYZ_NONEXISTENT".to_string() }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog_with_sales());
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].reason, RejectReason::Unmapped);
    assert_eq!(result[0].class, FieldClass::Measure);
}

#[test]
fn ac2_two_bad_measures_two_rejections() {
    let mqo = BoundMqoInput {
        measures: vec![
            MqoMeasureRef { unique_name: "Bad Measure A".to_string() },
            MqoMeasureRef { unique_name: "Bad Measure B".to_string() },
        ],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog_with_sales());
    assert_eq!(result.len(), 2);
    for rej in &result {
        assert_eq!(rej.reason, RejectReason::Unmapped);
        assert_eq!(rej.class, FieldClass::Measure);
    }
}
