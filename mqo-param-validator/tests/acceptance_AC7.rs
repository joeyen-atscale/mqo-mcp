//! AC7: cargo test passes; cargo clippy --all-targets -- -D warnings clean; zero unsafe.
//!
//! This test file provides a final integration smoke test that exercises
//! multiple validation rules together, confirming the crate compiles and
//! runs as a whole. The Clippy and no-unsafe requirements are enforced
//! by the build step (cargo clippy -- -D warnings) and by the #![forbid(unsafe_code)]
//! attribute in lib.rs (checked by the compiler itself at build time).

use mqo_param_validator::{
    validate, BoundMqoInput, CatalogDateRole, CatalogDimension, CatalogHierarchy, CatalogMeasure,
    CatalogSnapshot, FieldClass, MqoDimensionRef, MqoFilterRef, MqoMeasureRef, RejectReason,
};

fn full_catalog() -> CatalogSnapshot {
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
            CatalogDimension {
                unique_name: "Customer".to_string(),
                subject_areas: vec!["Sales".to_string()],
            },
            CatalogDimension {
                unique_name: "Date".to_string(),
                subject_areas: vec![],
            },
            CatalogDimension {
                unique_name: "Warehouse".to_string(),
                subject_areas: vec!["Inventory".to_string()],
            },
        ],
        hierarchies: vec![CatalogHierarchy {
            dimension_unique_name: "Customer".to_string(),
            hierarchy_unique_name: "Customer Hierarchy".to_string(),
            levels: vec![
                "Country".to_string(),
                "State".to_string(),
                "City".to_string(),
            ],
        }],
        date_roles: vec![
            CatalogDateRole {
                role_name: "[Order Date]".to_string(),
                base_dimension: "Date".to_string(),
            },
            CatalogDateRole {
                role_name: "[Ship Date]".to_string(),
                base_dimension: "Date".to_string(),
            },
        ],
    }
}

#[test]
fn ac7_valid_full_mqo_no_rejections() {
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Sales Amount".to_string(),
        }],
        dimensions: vec![
            MqoDimensionRef {
                unique_name: "Customer".to_string(),
                level: Some("City".to_string()),
                hierarchy: Some("Customer Hierarchy".to_string()),
                role_qualifier: None,
            },
            MqoDimensionRef {
                unique_name: "Date".to_string(),
                level: None,
                hierarchy: None,
                role_qualifier: Some("[Order Date]".to_string()),
            },
        ],
        filters: vec![],
    };
    let result = validate(&mqo, &full_catalog());
    assert!(result.is_empty(), "Full valid MQO should pass: {result:?}");
}

#[test]
fn ac7_multiple_violations_all_reported() {
    let mqo = BoundMqoInput {
        measures: vec![
            // AC2: unmapped measure
            MqoMeasureRef { unique_name: "Revenue Total".to_string() },
            // Valid
            MqoMeasureRef { unique_name: "Sales Amount".to_string() },
        ],
        dimensions: vec![
            // AC4: ambiguous date role
            MqoDimensionRef {
                unique_name: "Date".to_string(),
                level: None,
                hierarchy: None,
                role_qualifier: None,
            },
            // AC5: cross-fact (Sales Amount + Warehouse)
            MqoDimensionRef {
                unique_name: "Warehouse".to_string(),
                level: None,
                hierarchy: None,
                role_qualifier: None,
            },
        ],
        filters: vec![
            // unmapped filter
            MqoFilterRef { unique_name: "NonExistent".to_string(), level: None },
        ],
    };
    let result = validate(&mqo, &full_catalog());

    let unmapped_measures: Vec<_> = result
        .iter()
        .filter(|r| r.reason == RejectReason::Unmapped && r.class == FieldClass::Measure)
        .collect();
    assert_eq!(unmapped_measures.len(), 1, "One unmapped measure: {result:?}");

    let ambiguous: Vec<_> = result
        .iter()
        .filter(|r| r.reason == RejectReason::AmbiguousDateRole)
        .collect();
    assert_eq!(ambiguous.len(), 1, "One AmbiguousDateRole: {result:?}");

    let cross: Vec<_> = result
        .iter()
        .filter(|r| r.reason == RejectReason::CrossFactPath)
        .collect();
    assert_eq!(cross.len(), 1, "One CrossFactPath: {result:?}");

    let unmapped_filters: Vec<_> = result
        .iter()
        .filter(|r| r.reason == RejectReason::Unmapped && r.class == FieldClass::Filter)
        .collect();
    assert_eq!(unmapped_filters.len(), 1, "One unmapped filter: {result:?}");
}
