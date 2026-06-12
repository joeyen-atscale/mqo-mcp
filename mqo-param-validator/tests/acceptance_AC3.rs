//! AC3: An MQO referencing a level that exists in a different hierarchy
//! than the one its dimension selects is flagged WrongHierarchyLevel with
//! the chosen hierarchy's valid levels as suggestions.

use mqo_param_validator::{
    validate, BoundMqoInput, CatalogDimension, CatalogHierarchy, CatalogSnapshot, FieldClass,
    MqoDimensionRef, RejectReason,
};

fn catalog_with_geography() -> CatalogSnapshot {
    CatalogSnapshot {
        dimensions: vec![
            CatalogDimension { unique_name: "Geography".to_string(), subject_areas: vec![] },
        ],
        hierarchies: vec![
            CatalogHierarchy {
                dimension_unique_name: "Geography".to_string(),
                hierarchy_unique_name: "Country-State-City".to_string(),
                levels: vec!["Country".to_string(), "State".to_string(), "City".to_string()],
            },
            CatalogHierarchy {
                dimension_unique_name: "Geography".to_string(),
                hierarchy_unique_name: "Country-Region".to_string(),
                levels: vec!["Country".to_string(), "Region".to_string()],
            },
        ],
        ..Default::default()
    }
}

#[test]
fn ac3_wrong_level_for_chosen_hierarchy() {
    // "Region" exists in Country-Region but not in Country-State-City
    // If the MQO selects the Country-State-City hierarchy, "Region" is wrong
    let mqo = BoundMqoInput {
        dimensions: vec![MqoDimensionRef {
            unique_name: "Geography".to_string(),
            level: Some("Region".to_string()),
            hierarchy: Some("Country-State-City".to_string()),
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog_with_geography());
    assert_eq!(result.len(), 1, "Expected exactly one rejection: {result:?}");
    let rej = &result[0];
    assert_eq!(rej.class, FieldClass::HierarchyLevel);
    assert_eq!(rej.reason, RejectReason::WrongHierarchyLevel);
    assert_eq!(rej.field, "Region");
    // Suggestions should be the valid levels of Country-State-City
    let suggestion_names: Vec<&str> = rej.suggestions.iter().map(|s| s.name.as_str()).collect();
    assert!(suggestion_names.contains(&"Country"), "Should suggest Country");
    assert!(suggestion_names.contains(&"State"), "Should suggest State");
    assert!(suggestion_names.contains(&"City"), "Should suggest City");
}

#[test]
fn ac3_valid_level_in_chosen_hierarchy_no_rejection() {
    let mqo = BoundMqoInput {
        dimensions: vec![MqoDimensionRef {
            unique_name: "Geography".to_string(),
            level: Some("State".to_string()),
            hierarchy: Some("Country-State-City".to_string()),
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog_with_geography());
    assert!(
        result.is_empty(),
        "State is a valid level in Country-State-City: {result:?}"
    );
}

#[test]
fn ac3_no_hierarchy_specified_uses_first() {
    // Without hierarchy specified, validator picks first (Country-State-City)
    // "Region" is not in Country-State-City → WrongHierarchyLevel
    let mqo = BoundMqoInput {
        dimensions: vec![MqoDimensionRef {
            unique_name: "Geography".to_string(),
            level: Some("Region".to_string()),
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog_with_geography());
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].reason, RejectReason::WrongHierarchyLevel);
}
