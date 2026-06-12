//! AC4: When column_group sets intersect, bind() returns Bound (not Incompatible).

use mqo_catalog_binder::binder::{bind_with_compat, BindResult};
use mqo_catalog_binder::catalog::{CatalogSnapshot, ColumnEntry};
use mqo_catalog_binder::compat::EnrichedColumnGroups;
use mqo_spec::{LevelSelection, MeasureRef, Mqo};
use std::io::Write as _;

fn make_measure(unique_name: &str) -> ColumnEntry {
    ColumnEntry {
        unique_name: unique_name.to_string(),
        label: unique_name.to_string(),
        kind: "measure".to_string(),
        hierarchy: None,
        level: None,
        semi_additive: None,
        required_dimension: None,
        is_calc: false,
        ..Default::default()
    }
}

fn make_level(unique_name: &str, hierarchy: &str, level: &str) -> ColumnEntry {
    ColumnEntry {
        unique_name: unique_name.to_string(),
        label: level.to_string(),
        kind: "level".to_string(),
        hierarchy: Some(hierarchy.to_string()),
        level: Some(level.to_string()),
        semi_additive: None,
        required_dimension: None,
        is_calc: false,
        ..Default::default()
    }
}

fn write_enriched(entries: &[(&str, &[&str])]) -> tempfile::NamedTempFile {
    let columns: Vec<serde_json::Value> = entries
        .iter()
        .map(|(name, groups)| {
            serde_json::json!({ "unique_name": name, "column_group": groups })
        })
        .collect();
    let catalog = serde_json::json!({ "schema": "enriched-catalog.v1", "columns": columns });
    let mut f = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
    f.write_all(catalog.to_string().as_bytes()).unwrap();
    f
}

#[test]
fn ac4_intersecting_groups_returns_bound() {
    let snapshot = CatalogSnapshot {
        columns: vec![
            make_measure("sales.store_amount"),
            make_level("store.store_name.[Name]", "store.store_name", "Name"),
        ],
        ..CatalogSnapshot::default()
    };

    // Both belong to "store_sales" — intersection is non-empty → Bound.
    let enriched_file = write_enriched(&[
        ("sales.store_amount", &["store_sales"]),
        ("store.store_name.[Name]", &["store_sales"]),
    ]);
    let enriched = EnrichedColumnGroups::from_path(enriched_file.path()).unwrap();

    let mqo = Mqo {
        model: "tpcds".to_string(),
        measures: vec![MeasureRef {
            unique_name: "sales.store_amount".to_string(),
        }],
        dimensions: vec![LevelSelection {
            hierarchy: "store.store_name".to_string(),
            level: "Name".to_string(),
        }],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: None,
        non_empty: false,
    };

    match bind_with_compat(&mqo, &snapshot, &enriched) {
        BindResult::Bound(_) => {}
        other => panic!("expected Bound (intersecting groups), got {other:?}"),
    }
}

#[test]
fn ac4_partial_intersection_still_bound() {
    // Measure in {store_sales, catalog_sales}; dimension in {store_sales}.
    // Intersection = {store_sales} → Bound.
    let snapshot = CatalogSnapshot {
        columns: vec![
            make_measure("sales.amount"),
            make_level("store.dim.[City]", "store.dim", "City"),
        ],
        ..CatalogSnapshot::default()
    };
    let enriched_file = write_enriched(&[
        ("sales.amount", &["store_sales", "catalog_sales"]),
        ("store.dim.[City]", &["store_sales"]),
    ]);
    let enriched = EnrichedColumnGroups::from_path(enriched_file.path()).unwrap();

    let mqo = Mqo {
        model: "tpcds".to_string(),
        measures: vec![MeasureRef {
            unique_name: "sales.amount".to_string(),
        }],
        dimensions: vec![LevelSelection {
            hierarchy: "store.dim".to_string(),
            level: "City".to_string(),
        }],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: None,
        non_empty: false,
    };

    match bind_with_compat(&mqo, &snapshot, &enriched) {
        BindResult::Bound(_) => {}
        other => panic!("expected Bound (partial intersection), got {other:?}"),
    }
}
