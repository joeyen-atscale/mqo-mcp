//! AC5: A measure+dimension pair from clearly disjoint fact subject areas
//! returns CrossFactPath; a pair sharing a conformed dimension returns no such rejection.

use mqo_param_validator::{
    validate, BoundMqoInput, CatalogDimension, CatalogMeasure, CatalogSnapshot, MqoDimensionRef,
    MqoMeasureRef, RejectReason,
};

fn catalog_disjoint() -> CatalogSnapshot {
    CatalogSnapshot {
        measures: vec![
            CatalogMeasure {
                unique_name: "Sales Amount".to_string(),
                subject_area: Some("Sales".to_string()),
            },
            CatalogMeasure {
                unique_name: "Inventory Count".to_string(),
                subject_area: Some("Inventory".to_string()),
            },
        ],
        dimensions: vec![
            // This dimension is ONLY in the Inventory subject area
            CatalogDimension {
                unique_name: "Warehouse".to_string(),
                subject_areas: vec!["Inventory".to_string()],
            },
            // This is a conformed dimension (available everywhere)
            CatalogDimension {
                unique_name: "Date".to_string(),
                subject_areas: vec![],
            },
            // This is in Sales only
            CatalogDimension {
                unique_name: "Customer".to_string(),
                subject_areas: vec!["Sales".to_string()],
            },
        ],
        ..Default::default()
    }
}

#[test]
fn ac5_cross_fact_pair_rejected() {
    // Sales Amount (Sales SA) + Warehouse (Inventory SA) → CrossFactPath
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef { unique_name: "Sales Amount".to_string() }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "Warehouse".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog_disjoint());
    let cross: Vec<_> = result
        .iter()
        .filter(|r| r.reason == RejectReason::CrossFactPath)
        .collect();
    assert_eq!(cross.len(), 1, "Expected exactly one CrossFactPath: {result:?}");
    assert!(
        cross[0].field.contains("Sales Amount"),
        "CrossFactPath field should reference measure: {}",
        cross[0].field
    );
    assert!(
        cross[0].field.contains("Warehouse"),
        "CrossFactPath field should reference dimension: {}",
        cross[0].field
    );
}

#[test]
fn ac5_conformed_dimension_no_false_positive() {
    // Sales Amount + Date (conformed, subject_areas=[]) → no CrossFactPath
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef { unique_name: "Sales Amount".to_string() }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "Date".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog_disjoint());
    let cross: Vec<_> = result
        .iter()
        .filter(|r| r.reason == RejectReason::CrossFactPath)
        .collect();
    assert!(
        cross.is_empty(),
        "Conformed dimension should never trigger CrossFactPath: {result:?}"
    );
}

#[test]
fn ac5_same_subject_area_no_rejection() {
    // Sales Amount (Sales) + Customer (Sales) → no CrossFactPath
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef { unique_name: "Sales Amount".to_string() }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "Customer".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog_disjoint());
    let cross: Vec<_> = result
        .iter()
        .filter(|r| r.reason == RejectReason::CrossFactPath)
        .collect();
    assert!(
        cross.is_empty(),
        "Same subject area should not trigger CrossFactPath: {result:?}"
    );
}

#[test]
fn ac5_measure_without_subject_area_no_rejection() {
    // Measure with no subject_area set → conservative, no CrossFactPath
    let catalog = CatalogSnapshot {
        measures: vec![CatalogMeasure {
            unique_name: "Sales Amount".to_string(),
            subject_area: None, // unknown
        }],
        dimensions: vec![CatalogDimension {
            unique_name: "Warehouse".to_string(),
            subject_areas: vec!["Inventory".to_string()],
        }],
        ..Default::default()
    };
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef { unique_name: "Sales Amount".to_string() }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "Warehouse".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog);
    let cross: Vec<_> = result
        .iter()
        .filter(|r| r.reason == RejectReason::CrossFactPath)
        .collect();
    assert!(
        cross.is_empty(),
        "No subject_area on measure → conservative, no CrossFactPath: {result:?}"
    );
}
