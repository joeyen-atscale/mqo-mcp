#![allow(clippy::expect_used, clippy::missing_const_for_fn)]
//! AC4: Unbound entities carry `column_group`: [] and appear in coverage report.

use mqoguard_column_group_enrichment::{enrich, CatalogColumn, CatalogSnapshot, FactBindings};
use std::collections::BTreeMap;

fn empty_bindings() -> FactBindings {
    FactBindings {
        measures: BTreeMap::new(),
        hierarchies: BTreeMap::new(),
    }
}

#[test]
fn ac4_unbound_measure_not_dropped() {
    let catalog = CatalogSnapshot {
        columns: vec![CatalogColumn {
            unique_name: "model.unbound_measure".to_string(),
            kind: Some("measure".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };

    let enriched = enrich(&catalog, &empty_bindings());

    assert_eq!(enriched.columns.len(), 1, "AC4: column must not be dropped");
    let col = enriched.columns.first().expect("one column");
    assert!(col.column_group.is_empty(), "AC4: unbound entity must have empty column_group");
    assert!(
        enriched
            .coverage
            .unbound
            .contains(&"model.unbound_measure".to_string()),
        "AC4: unbound entity must appear in coverage.unbound"
    );
    assert_eq!(enriched.coverage.unbound_count, 1);
}

#[test]
fn ac4_unbound_level_not_dropped() {
    let catalog = CatalogSnapshot {
        columns: vec![CatalogColumn {
            unique_name: "dim.unknown.[Field]".to_string(),
            kind: Some("level".to_string()),
            hierarchy: Some("dim.unknown".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };

    let enriched = enrich(&catalog, &empty_bindings());

    assert_eq!(enriched.columns.len(), 1);
    let col = enriched.columns.first().expect("one column");
    assert!(col.column_group.is_empty());
    assert!(enriched
        .coverage
        .unbound
        .contains(&"dim.unknown.[Field]".to_string()));
}

#[test]
fn ac4_mixed_bound_and_unbound() {
    let mut bindings = empty_bindings();
    bindings
        .measures
        .insert("model.bound".to_string(), std::iter::once("sales".to_string()).collect());

    let catalog = CatalogSnapshot {
        columns: vec![
            CatalogColumn {
                unique_name: "model.bound".to_string(),
                kind: Some("measure".to_string()),
                ..Default::default()
            },
            CatalogColumn {
                unique_name: "model.unbound".to_string(),
                kind: Some("measure".to_string()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let enriched = enrich(&catalog, &bindings);

    assert_eq!(enriched.columns.len(), 2);
    assert_eq!(enriched.coverage.bound, 1);
    assert_eq!(enriched.coverage.unbound_count, 1);
    // 0.5 is exactly representable in IEEE 754 — no float-equality hazard
    assert!(
        (enriched.coverage.coverage_pct - 0.5_f64).abs() < f64::EPSILON,
        "coverage_pct must be 0.5 when half are unbound, got {}",
        enriched.coverage.coverage_pct
    );
    assert!(enriched
        .coverage
        .unbound
        .contains(&"model.unbound".to_string()));
}

#[test]
fn ac4_coverage_report_totals_match_column_count() {
    let catalog = CatalogSnapshot {
        columns: (0..5)
            .map(|i| CatalogColumn {
                unique_name: format!("model.col{i}"),
                kind: Some("measure".to_string()),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };

    let enriched = enrich(&catalog, &empty_bindings());

    assert_eq!(enriched.coverage.total, 5);
    assert_eq!(enriched.coverage.unbound_count, 5);
    assert_eq!(enriched.coverage.bound, 0);
    assert!(
        enriched.coverage.coverage_pct.abs() < f64::EPSILON,
        "coverage_pct must be 0.0 when all unbound, got {}",
        enriched.coverage.coverage_pct
    );
}
