//! AC2: Without --enriched-catalog, bind() returns Bound (legacy behavior unchanged).

use mqo_catalog_binder::binder::{bind, BindResult};
use mqo_catalog_binder::catalog::{CatalogSnapshot, ColumnEntry};
use mqo_spec::{LevelSelection, MeasureRef, Mqo};

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

#[test]
fn ac2_no_enriched_catalog_returns_bound() {
    // Same cross-fact scenario as AC1 — but we call bind() without enriched catalog.
    let snapshot = CatalogSnapshot {
        columns: vec![
            make_measure("sales.store_amount"),
            make_level("returns.reason.[Reason]", "returns.reason", "Reason"),
        ],
        ..CatalogSnapshot::default()
    };

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

    // Without enriched catalog: must return Bound (identical to pre-extension behavior).
    match bind(&mqo, &snapshot) {
        BindResult::Bound(bound) => {
            assert_eq!(bound.measures[0].unique_name, "sales.store_amount");
            assert_eq!(bound.dimensions[0].unique_name, "returns.reason.[Reason]");
        }
        other => panic!("expected Bound (legacy behavior), got {other:?}"),
    }
}
