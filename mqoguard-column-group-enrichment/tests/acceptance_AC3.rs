#![allow(clippy::expect_used, clippy::missing_const_for_fn)]
//! AC3: Every non-`column_group` field is present and byte-identical in the output.

use mqoguard_column_group_enrichment::{enrich, CatalogColumn, CatalogSnapshot, FactBindings};
use std::collections::BTreeMap;

fn full_column(unique_name: &str) -> CatalogColumn {
    let mut extra = BTreeMap::new();
    extra.insert(
        "custom_field".to_string(),
        serde_json::json!("preserved_value"),
    );
    extra.insert("numeric_extra".to_string(), serde_json::json!(42));
    CatalogColumn {
        unique_name: unique_name.to_string(),
        label: Some("My Label".to_string()),
        kind: Some("measure".to_string()),
        hierarchy: None,
        level: None,
        is_calc: Some(true),
        extra,
    }
}

#[test]
fn ac3_all_fields_preserved_for_measure() {
    let input = full_column("model.m1");
    let catalog = CatalogSnapshot {
        catalog: Some("test_catalog".to_string()),
        schema: Some("test_schema".to_string()),
        columns: vec![input.clone()],
        ..Default::default()
    };
    let bindings = FactBindings {
        measures: BTreeMap::new(),
        hierarchies: BTreeMap::new(),
    };

    let enriched = enrich(&catalog, &bindings);

    let out = enriched.columns.first().expect("one column");
    assert_eq!(out.unique_name, input.unique_name, "unique_name preserved");
    assert_eq!(out.label, input.label, "label preserved");
    assert_eq!(out.kind, input.kind, "kind preserved");
    assert_eq!(out.hierarchy, input.hierarchy, "hierarchy preserved");
    assert_eq!(out.level, input.level, "level preserved");
    assert_eq!(out.is_calc, input.is_calc, "is_calc preserved");
    assert_eq!(
        out.extra.get("custom_field"),
        Some(&serde_json::json!("preserved_value")),
        "extra fields preserved"
    );
    assert_eq!(
        out.extra.get("numeric_extra"),
        Some(&serde_json::json!(42)),
        "numeric extra preserved"
    );
}

#[test]
fn ac3_level_fields_preserved() {
    let col = CatalogColumn {
        unique_name: "dim.date.[Year]".to_string(),
        label: Some("Year".to_string()),
        kind: Some("level".to_string()),
        hierarchy: Some("dim.date".to_string()),
        level: Some("Year".to_string()),
        is_calc: Some(false),
        extra: BTreeMap::new(),
    };
    let catalog = CatalogSnapshot {
        columns: vec![col.clone()],
        ..Default::default()
    };
    let bindings = FactBindings {
        measures: BTreeMap::new(),
        hierarchies: BTreeMap::new(),
    };

    let enriched = enrich(&catalog, &bindings);
    let out = enriched.columns.first().expect("one column");

    assert_eq!(out.unique_name, col.unique_name);
    assert_eq!(out.label, col.label);
    assert_eq!(out.kind, col.kind);
    assert_eq!(out.hierarchy, col.hierarchy);
    assert_eq!(out.level, col.level);
    assert_eq!(out.is_calc, col.is_calc);
}

#[test]
fn ac3_column_order_preserved() {
    let catalog = CatalogSnapshot {
        columns: (0..10)
            .map(|i| CatalogColumn {
                unique_name: format!("model.m{i}"),
                kind: Some("measure".to_string()),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let bindings = FactBindings {
        measures: BTreeMap::new(),
        hierarchies: BTreeMap::new(),
    };

    let enriched = enrich(&catalog, &bindings);
    for (i, col) in enriched.columns.iter().enumerate() {
        assert_eq!(
            col.unique_name,
            format!("model.m{i}"),
            "order must be preserved at index {i}"
        );
    }
}
