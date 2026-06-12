//! AC1: An MQO whose every measure and dimension has an exact
//! (post-normalization) unique_name in the CatalogSnapshot returns an empty Vec<ParamRejection>.

use mqo_param_validator::{
    validate, BoundMqoInput, CatalogDimension, CatalogMeasure, CatalogSnapshot, MqoDimensionRef,
    MqoMeasureRef,
};

fn catalog() -> CatalogSnapshot {
    CatalogSnapshot {
        measures: vec![
            CatalogMeasure { unique_name: "Sales Amount".to_string(), subject_area: None },
            CatalogMeasure { unique_name: "Order Count".to_string(), subject_area: None },
        ],
        dimensions: vec![
            CatalogDimension { unique_name: "Customer".to_string(), subject_areas: vec![] },
            CatalogDimension { unique_name: "Product".to_string(), subject_areas: vec![] },
        ],
        hierarchies: vec![],
        date_roles: vec![],
    }
}

#[test]
fn ac1_all_resolved_returns_empty() {
    let mqo = BoundMqoInput {
        measures: vec![
            MqoMeasureRef { unique_name: "Sales Amount".to_string() },
        ],
        dimensions: vec![
            MqoDimensionRef {
                unique_name: "Customer".to_string(),
                level: None,
                hierarchy: None,
                role_qualifier: None,
            },
        ],
        filters: vec![],
    };
    let result = validate(&mqo, &catalog());
    assert!(
        result.is_empty(),
        "Expected no rejections for a fully-mapped MQO, got: {result:?}"
    );
}

#[test]
fn ac1_normalization_case_insensitive() {
    // Catalog has "Sales Amount" but MQO sends "sales amount" — should still resolve
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef { unique_name: "sales amount".to_string() }],
        dimensions: vec![],
        filters: vec![],
    };
    let result = validate(&mqo, &catalog());
    assert!(result.is_empty(), "Normalization should handle case differences: {result:?}");
}

#[test]
fn ac1_empty_mqo_returns_empty() {
    let result = validate(&BoundMqoInput::default(), &catalog());
    assert!(result.is_empty());
}

#[test]
fn ac1_empty_catalog_empty_mqo_returns_empty() {
    let result = validate(&BoundMqoInput::default(), &CatalogSnapshot::default());
    assert!(result.is_empty());
}
