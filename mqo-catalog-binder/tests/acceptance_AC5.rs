//! AC5: NotFound and Ambiguous take precedence over Incompatible.

use mqo_catalog_binder::binder::{bind_with_compat, BindResult};
use mqo_catalog_binder::catalog::{CatalogSnapshot, ColumnEntry}; // ColumnEntry used in ambiguous test
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
fn ac5_not_found_measure_with_cross_fact_dim_returns_not_found() {
    let snapshot = CatalogSnapshot {
        columns: vec![
            // The dimension resolves fine.
            make_level("returns.reason.[Reason]", "returns.reason", "Reason"),
            // The measure "NonExistent" is NOT in the catalog.
        ],
        ..CatalogSnapshot::default()
    };

    // Even if the dimension has a cross-fact group, the unresolvable measure
    // means NotFound takes precedence over Incompatible.
    let enriched_file = write_enriched(&[
        ("returns.reason.[Reason]", &["catalog_returns"]),
    ]);
    let enriched = EnrichedColumnGroups::from_path(enriched_file.path()).unwrap();

    let mqo = Mqo {
        model: "tpcds".to_string(),
        measures: vec![MeasureRef {
            unique_name: "NonExistentMeasureXYZ".to_string(),
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
        BindResult::NotFound(refs) => {
            assert!(
                refs.iter().any(|r| r.contains("NonExistentMeasureXYZ")),
                "not_found must name the missing ref"
            );
        }
        other => panic!("expected NotFound (precedence over Incompatible), got {other:?}"),
    }
}

#[test]
fn ac5_ambiguous_measure_returns_ambiguous_not_incompatible() {
    // Two measures with the SAME label "Revenue" — ambiguous label collision.
    // (make_measure sets label = unique_name; use inline ColumnEntry to set matching labels.)
    let snapshot = CatalogSnapshot {
        columns: vec![
            ColumnEntry {
                unique_name: "model_a.revenue".to_string(),
                label: "Revenue".to_string(), // same label
                kind: "measure".to_string(),
                hierarchy: None,
                level: None,
                semi_additive: None,
                required_dimension: None,
                is_calc: false,
                ..Default::default()
            },
            ColumnEntry {
                unique_name: "model_b.revenue".to_string(),
                label: "Revenue".to_string(), // same label
                kind: "measure".to_string(),
                hierarchy: None,
                level: None,
                semi_additive: None,
                required_dimension: None,
                is_calc: false,
                ..Default::default()
            },
            make_level("returns.reason.[Reason]", "returns.reason", "Reason"),
        ],
        ..CatalogSnapshot::default()
    };

    let enriched_file = write_enriched(&[
        ("model_a.revenue", &["store_sales"]),
        ("model_b.revenue", &["store_sales"]),
        ("returns.reason.[Reason]", &["catalog_returns"]),
    ]);
    let enriched = EnrichedColumnGroups::from_path(enriched_file.path()).unwrap();

    // Label "Revenue" (case-insensitive) matches both → Ambiguous before Incompatible.
    let mqo = Mqo {
        model: "tpcds".to_string(),
        measures: vec![MeasureRef {
            unique_name: "Revenue".to_string(),
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
        BindResult::Ambiguous(_) => {}
        other => panic!("expected Ambiguous (precedence over Incompatible), got {other:?}"),
    }
}
