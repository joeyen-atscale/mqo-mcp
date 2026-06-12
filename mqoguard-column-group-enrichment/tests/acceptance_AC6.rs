#![allow(clippy::expect_used, clippy::missing_const_for_fn)]
//! AC6: Malformed JSON returns typed error or empty result — never panics.

use mqoguard_column_group_enrichment::{CatalogSnapshot, FactBindings, FactBindingsError};

#[test]
fn ac6_malformed_catalog_json_returns_default_not_panic() {
    let result: Result<CatalogSnapshot, _> = serde_json::from_str("not json at all");
    assert!(result.is_err(), "AC6: malformed catalog JSON must return Err");
}

#[test]
fn ac6_empty_object_catalog_parses_to_empty_columns() {
    let result: Result<CatalogSnapshot, _> = serde_json::from_str("{}");
    assert!(result.is_ok(), "AC6: empty object must parse successfully");
    let catalog = result.unwrap_or_default();
    assert!(catalog.columns.is_empty());
}

#[test]
fn ac6_malformed_bindings_json_returns_error() {
    let result = FactBindings::from_json("{{broken");
    assert!(
        matches!(result, Err(FactBindingsError::Json(_))),
        "AC6: malformed bindings JSON must return FactBindingsError::Json"
    );
}

#[test]
fn ac6_empty_bindings_json_returns_empty_error() {
    let result = FactBindings::from_json(r#"{"measures": {}, "hierarchies": {}}"#);
    assert!(
        matches!(result, Err(FactBindingsError::Empty)),
        "AC6: empty bindings must return FactBindingsError::Empty"
    );
}

#[test]
fn ac6_columns_with_null_kind_do_not_panic() {
    // A column with null/missing kind must not panic — produces unbound column
    use mqoguard_column_group_enrichment::{enrich, CatalogColumn, CatalogSnapshot, FactBindings};
    use std::collections::BTreeMap;

    let col = CatalogColumn {
        unique_name: "model.weird".to_string(),
        kind: None, // no kind field
        ..Default::default()
    };
    let catalog = CatalogSnapshot {
        columns: vec![col],
        ..Default::default()
    };
    let bindings = FactBindings {
        measures: BTreeMap::new(),
        hierarchies: BTreeMap::new(),
    };
    // Must not panic
    let enriched = enrich(&catalog, &bindings);
    assert_eq!(enriched.columns.len(), 1);
    let c = enriched.columns.first().expect("one column");
    assert!(c.column_group.is_empty());
}

#[test]
fn ac6_level_with_no_hierarchy_does_not_panic() {
    use mqoguard_column_group_enrichment::{enrich, CatalogColumn, CatalogSnapshot, FactBindings};
    use std::collections::BTreeMap;

    let col = CatalogColumn {
        unique_name: "dim.foo.[Bar]".to_string(),
        kind: Some("level".to_string()),
        hierarchy: None, // missing hierarchy
        ..Default::default()
    };
    let catalog = CatalogSnapshot {
        columns: vec![col],
        ..Default::default()
    };
    let bindings = FactBindings {
        measures: BTreeMap::new(),
        hierarchies: BTreeMap::new(),
    };
    let enriched = enrich(&catalog, &bindings);
    assert_eq!(enriched.columns.len(), 1);
    let c = enriched.columns.first().expect("one column");
    assert!(c.column_group.is_empty());
}

#[test]
fn ac6_array_instead_of_object_for_catalog_returns_error() {
    let result: Result<mqoguard_column_group_enrichment::CatalogSnapshot, _> =
        serde_json::from_str("[1, 2, 3]");
    assert!(result.is_err(), "AC6: array JSON must not parse as catalog");
}
