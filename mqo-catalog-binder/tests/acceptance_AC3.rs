//! AC3: Conformed dimensions (empty column_group or wildcard "*") are never flagged.

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

fn cross_fact_mqo() -> (CatalogSnapshot, Mqo) {
    let snapshot = CatalogSnapshot {
        columns: vec![
            make_measure("sales.store_amount"),
            make_level("time.calendar.[Year]", "time.calendar", "Year"),
        ],
        ..CatalogSnapshot::default()
    };
    let mqo = Mqo {
        model: "tpcds".to_string(),
        measures: vec![MeasureRef {
            unique_name: "sales.store_amount".to_string(),
        }],
        dimensions: vec![LevelSelection {
            hierarchy: "time.calendar".to_string(),
            level: "Year".to_string(),
        }],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: None,
        non_empty: false,
    };
    (snapshot, mqo)
}

#[test]
fn ac3_conformed_dim_empty_group_not_flagged() {
    let (snapshot, mqo) = cross_fact_mqo();

    // Measure has a non-empty group; dimension has an EMPTY group (conformed).
    let enriched_file = write_enriched(&[
        ("sales.store_amount", &["store_sales"]),
        ("time.calendar.[Year]", &[]), // empty = conformed
    ]);
    let enriched = EnrichedColumnGroups::from_path(enriched_file.path()).unwrap();

    match bind_with_compat(&mqo, &snapshot, &enriched) {
        BindResult::Bound(_) => {} // correct — conformed dimension never flagged
        other => panic!("expected Bound (empty-set conformed dim), got {other:?}"),
    }
}

#[test]
fn ac3_conformed_dim_wildcard_not_flagged() {
    let (snapshot, mqo) = cross_fact_mqo();

    // Dimension carries wildcard "*" marker.
    let enriched_file = write_enriched(&[
        ("sales.store_amount", &["store_sales"]),
        ("time.calendar.[Year]", &["*"]),
    ]);
    let enriched = EnrichedColumnGroups::from_path(enriched_file.path()).unwrap();

    match bind_with_compat(&mqo, &snapshot, &enriched) {
        BindResult::Bound(_) => {} // correct — wildcard-conformed dimension never flagged
        other => panic!("expected Bound (wildcard conformed dim), got {other:?}"),
    }
}
