//! AC4: A date concept resolving to ≥2 role-played date dimensions
//! with no role qualifier returns AmbiguousDateRole listing every candidate role.

use mqo_param_validator::{
    validate, BoundMqoInput, CatalogDateRole, CatalogDimension, CatalogSnapshot, FieldClass,
    MqoDimensionRef, RejectReason,
};

fn catalog_with_date_roles() -> CatalogSnapshot {
    CatalogSnapshot {
        dimensions: vec![
            CatalogDimension { unique_name: "Date".to_string(), subject_areas: vec![] },
        ],
        date_roles: vec![
            CatalogDateRole {
                role_name: "[Order Date]".to_string(),
                base_dimension: "Date".to_string(),
            },
            CatalogDateRole {
                role_name: "[Ship Date]".to_string(),
                base_dimension: "Date".to_string(),
            },
            CatalogDateRole {
                role_name: "[Return Date]".to_string(),
                base_dimension: "Date".to_string(),
            },
        ],
        ..Default::default()
    }
}

#[test]
fn ac4_ambiguous_date_no_qualifier_rejected() {
    // "Date" base dimension with no role qualifier → AmbiguousDateRole
    let mqo = BoundMqoInput {
        dimensions: vec![MqoDimensionRef {
            unique_name: "Date".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog_with_date_roles());
    assert_eq!(result.len(), 1, "Expected one AmbiguousDateRole rejection: {result:?}");
    let rej = &result[0];
    assert_eq!(rej.class, FieldClass::DateRole);
    assert_eq!(rej.reason, RejectReason::AmbiguousDateRole);
    assert_eq!(rej.field, "Date");
    // All three roles must appear in suggestions
    let sugg_names: Vec<&str> = rej.suggestions.iter().map(|s| s.name.as_str()).collect();
    assert!(sugg_names.contains(&"[Order Date]"), "Should list [Order Date]");
    assert!(sugg_names.contains(&"[Ship Date]"), "Should list [Ship Date]");
    assert!(sugg_names.contains(&"[Return Date]"), "Should list [Return Date]");
}

#[test]
fn ac4_with_role_qualifier_no_rejection() {
    // When a role qualifier is given, no AmbiguousDateRole
    let mqo = BoundMqoInput {
        dimensions: vec![MqoDimensionRef {
            unique_name: "Date".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: Some("[Order Date]".to_string()),
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog_with_date_roles());
    let ambiguous: Vec<_> = result
        .iter()
        .filter(|r| r.reason == RejectReason::AmbiguousDateRole)
        .collect();
    assert!(ambiguous.is_empty(), "With qualifier, no AmbiguousDateRole expected: {result:?}");
}

#[test]
fn ac4_single_role_no_rejection() {
    // Only one role for this base_dimension → no ambiguity
    let catalog = CatalogSnapshot {
        dimensions: vec![
            CatalogDimension { unique_name: "Date".to_string(), subject_areas: vec![] },
        ],
        date_roles: vec![CatalogDateRole {
            role_name: "[Order Date]".to_string(),
            base_dimension: "Date".to_string(),
        }],
        ..Default::default()
    };
    let mqo = BoundMqoInput {
        dimensions: vec![MqoDimensionRef {
            unique_name: "Date".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog);
    let ambiguous: Vec<_> = result
        .iter()
        .filter(|r| r.reason == RejectReason::AmbiguousDateRole)
        .collect();
    assert!(ambiguous.is_empty(), "Single role — no AmbiguousDateRole: {result:?}");
}
