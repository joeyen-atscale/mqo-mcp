//! AC1: Cross-fact measure×dimension pair → Incompatible with one report.

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
fn ac1_cross_fact_returns_incompatible_with_one_report() {
    let snapshot = CatalogSnapshot {
        columns: vec![
            make_measure("sales.store_amount"),
            make_level("returns.reason.[Reason]", "returns.reason", "Reason"),
        ],
        ..CatalogSnapshot::default()
    };

    let enriched_file = write_enriched(&[
        ("sales.store_amount", &["store_sales"]),
        ("returns.reason.[Reason]", &["catalog_returns"]),
    ]);
    let enriched = EnrichedColumnGroups::from_path(enriched_file.path()).unwrap();

    let mqo = Mqo {
        model: "tpcds".to_string(),
        measures: vec![MeasureRef {
            unique_name: "sales.store_amount".to_string(),
        }],
        dimensions: vec![LevelSelection {
            hierarchy: "returns.reason".to_string(),
            level: "Reason".to_string(),
        }],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: None,
        non_empty: false,
        projection: false,
        };

    match bind_with_compat(&mqo, &snapshot, &enriched) {
        BindResult::Incompatible(reports) => {
            assert_eq!(reports.len(), 1, "expected exactly one incompatibility report");
            let r = &reports[0];
            assert_eq!(r.measure_unique_name, "sales.store_amount");
            assert_eq!(r.dimension_unique_name, "returns.reason.[Reason]");
            assert_eq!(r.measure_column_groups, vec!["store_sales"]);
            assert_eq!(r.dimension_column_groups, vec!["catalog_returns"]);
            assert!(!r.note.is_empty(), "note must be non-empty");
        }
        other => panic!("expected Incompatible, got {other:?}"),
    }
}
