//! AC7: Measure/dimension absent from enriched catalog → treated as conformed → Bound (fail-open).

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
fn ac7_measure_absent_from_enriched_catalog_not_flagged() {
    let snapshot = CatalogSnapshot {
        columns: vec![
            make_measure("sales.store_amount"),
            make_level("returns.reason.[Reason]", "returns.reason", "Reason"),
        ],
        ..CatalogSnapshot::default()
    };

    // Enriched catalog has only the dimension entry; measure is absent (fail-open).
    let enriched_file = write_enriched(&[
        ("returns.reason.[Reason]", &["catalog_returns"]),
        // "sales.store_amount" is intentionally absent
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
    };

    // Absent measure treated as conformed (empty set) → never flagged → Bound.
    match bind_with_compat(&mqo, &snapshot, &enriched) {
        BindResult::Bound(_) => {}
        other => panic!("expected Bound (absent measure = conformed fail-open), got {other:?}"),
    }
}

#[test]
fn ac7_dimension_absent_from_enriched_catalog_not_flagged() {
    let snapshot = CatalogSnapshot {
        columns: vec![
            make_measure("sales.store_amount"),
            make_level("returns.reason.[Reason]", "returns.reason", "Reason"),
        ],
        ..CatalogSnapshot::default()
    };

    // Enriched catalog has only the measure entry; dimension is absent (fail-open).
    let enriched_file = write_enriched(&[
        ("sales.store_amount", &["store_sales"]),
        // "returns.reason.[Reason]" is intentionally absent
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
    };

    // Absent dimension treated as conformed → never flagged → Bound.
    match bind_with_compat(&mqo, &snapshot, &enriched) {
        BindResult::Bound(_) => {}
        other => panic!("expected Bound (absent dim = conformed fail-open), got {other:?}"),
    }
}

#[test]
fn ac7_both_absent_from_enriched_catalog_not_flagged() {
    let snapshot = CatalogSnapshot {
        columns: vec![
            make_measure("sales.store_amount"),
            make_level("returns.reason.[Reason]", "returns.reason", "Reason"),
        ],
        ..CatalogSnapshot::default()
    };

    // Completely empty enriched catalog — nothing enriched → everything conformed.
    let enriched_file = write_enriched(&[]);
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
    };

    match bind_with_compat(&mqo, &snapshot, &enriched) {
        BindResult::Bound(_) => {}
        other => panic!("expected Bound (both absent = conformed), got {other:?}"),
    }
}
