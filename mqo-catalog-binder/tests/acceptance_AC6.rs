//! AC6: Multiple cross-fact pairs → Incompatible with one report per pair, deterministic order.

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
fn ac6_two_measures_one_cross_fact_dimension_two_reports() {
    // Two measures (store_sales, catalog_sales) × one cross-fact dimension (web_returns).
    let snapshot = CatalogSnapshot {
        columns: vec![
            make_measure("sales.store_amount"),
            make_measure("sales.catalog_amount"),
            make_level("returns.web.[Channel]", "returns.web", "Channel"),
        ],
        ..CatalogSnapshot::default()
    };

    let enriched_file = write_enriched(&[
        ("sales.store_amount", &["store_sales"]),
        ("sales.catalog_amount", &["catalog_sales"]),
        ("returns.web.[Channel]", &["web_returns"]),
    ]);
    let enriched = EnrichedColumnGroups::from_path(enriched_file.path()).unwrap();

    let mqo = Mqo {
        model: "tpcds".to_string(),
        measures: vec![
            MeasureRef { unique_name: "sales.store_amount".to_string() },
            MeasureRef { unique_name: "sales.catalog_amount".to_string() },
        ],
        dimensions: vec![LevelSelection {
            hierarchy: "returns.web".to_string(),
            level: "Channel".to_string(),
        }],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: None,
        non_empty: false,
    };

    match bind_with_compat(&mqo, &snapshot, &enriched) {
        BindResult::Incompatible(reports) => {
            assert_eq!(reports.len(), 2, "expected one report per offending pair");
            // Deterministic order: measure unique_name ascending.
            assert!(
                reports[0].measure_unique_name <= reports[1].measure_unique_name,
                "reports must be sorted by measure_unique_name"
            );
        }
        other => panic!("expected Incompatible, got {other:?}"),
    }
}

#[test]
fn ac6_one_measure_two_cross_fact_dimensions_two_reports() {
    let snapshot = CatalogSnapshot {
        columns: vec![
            make_measure("sales.store_amount"),
            make_level("returns.store.[Reason]", "returns.store", "Reason"),
            make_level("returns.catalog.[Channel]", "returns.catalog", "Channel"),
        ],
        ..CatalogSnapshot::default()
    };

    let enriched_file = write_enriched(&[
        ("sales.store_amount", &["store_sales"]),
        ("returns.store.[Reason]", &["store_returns"]),
        ("returns.catalog.[Channel]", &["catalog_returns"]),
    ]);
    let enriched = EnrichedColumnGroups::from_path(enriched_file.path()).unwrap();

    let mqo = Mqo {
        model: "tpcds".to_string(),
        measures: vec![MeasureRef {
            unique_name: "sales.store_amount".to_string(),
        }],
        dimensions: vec![
            LevelSelection { hierarchy: "returns.store".to_string(), level: "Reason".to_string() },
            LevelSelection { hierarchy: "returns.catalog".to_string(), level: "Channel".to_string() },
        ],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: None,
        non_empty: false,
    };

    match bind_with_compat(&mqo, &snapshot, &enriched) {
        BindResult::Incompatible(reports) => {
            assert_eq!(reports.len(), 2, "expected one report per offending dimension");
            // Deterministic secondary sort: dimension unique_name ascending.
            assert!(
                reports[0].dimension_unique_name <= reports[1].dimension_unique_name,
                "reports must be sorted by dimension_unique_name when measure is the same"
            );
        }
        other => panic!("expected Incompatible, got {other:?}"),
    }
}
