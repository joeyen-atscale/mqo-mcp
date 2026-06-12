//! AC6 (SHOULD): BoundMqo carries resolved unique_names + per-ref metadata flags.

use mqo_spec::{BoundMqo, BoundMeasure, BoundDimension, Mqo, MeasureRef, LevelSelection};

fn make_bound_mqo() -> BoundMqo {
    let mqo = Mqo {
        model: "sales".to_string(),
        measures: vec![MeasureRef {
            unique_name: "sales.revenue".to_string(),
        }],
        dimensions: vec![LevelSelection {
            hierarchy: "date.calendar".to_string(),
            level: "year".to_string(),
        }],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: None,
        non_empty: true,
    };

    BoundMqo {
        mqo,
        measures: vec![BoundMeasure {
            unique_name: "[Measures].[Revenue]".to_string(),
            is_calc: false,
            semi_additive: false,
            required_dimension: None,
        }],
        dimensions: vec![BoundDimension {
            unique_name: "[Date].[Calendar].[Year]".to_string(),
            hierarchy: "[Date].[Calendar]".to_string(),
        }],
    }
}

#[test]
fn bound_mqo_fields_present() {
    let bound = make_bound_mqo();
    assert_eq!(bound.measures.len(), 1);
    assert_eq!(bound.dimensions.len(), 1);

    let m = &bound.measures[0];
    assert!(!m.unique_name.is_empty(), "unique_name must be non-empty");
    // is_calc and semi_additive are bool — just confirm they're accessible
    let _ = m.is_calc;
    let _ = m.semi_additive;
    let _ = &m.required_dimension;
}

#[test]
fn bound_mqo_round_trips_json() {
    let bound = make_bound_mqo();
    let json = serde_json::to_string(&bound).expect("serialise BoundMqo");
    let reparsed: BoundMqo = serde_json::from_str(&json).expect("deserialise BoundMqo");
    assert_eq!(bound, reparsed);
}

#[test]
fn bound_mqo_with_semi_additive_measure() {
    let mqo = Mqo {
        model: "finance".to_string(),
        measures: vec![MeasureRef {
            unique_name: "finance.headcount".to_string(),
        }],
        dimensions: vec![],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: None,
        non_empty: false,
    };

    let bound = BoundMqo {
        mqo,
        measures: vec![BoundMeasure {
            unique_name: "[Measures].[Headcount]".to_string(),
            is_calc: false,
            semi_additive: true,
            required_dimension: Some("[Date].[Calendar].[Year]".to_string()),
        }],
        dimensions: vec![],
    };

    assert!(bound.measures[0].semi_additive);
    assert!(bound.measures[0].required_dimension.is_some());
}
