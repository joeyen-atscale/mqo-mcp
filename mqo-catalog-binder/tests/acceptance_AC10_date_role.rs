//! AC10 (PRD mqo-crossfact-date-role-binding): cross-fact per-measure date-role
//! binding and pre-execution `cross_fact_date_incompatible` rejection.
//!
//! Mirrors corpus task `tpcds-fm1-009` ("Show monthly inventory on hand alongside
//! store sales for 2001") — inventory measure needs Inventory Calendar Month,
//! store sales needs Sold Calendar Month.

use mqo_catalog_binder::binder::{bind_with_date_roles, BindResult};
use mqo_catalog_binder::catalog::{CatalogSnapshot, ColumnEntry};
use mqo_catalog_binder::compat::EnrichedColumnGroups;
use mqo_spec::{LevelSelection, MeasureRef, Mqo};
use std::io::Write as _;

fn measure(unique_name: &str) -> ColumnEntry {
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

fn level(unique_name: &str, hierarchy: &str, lvl: &str) -> ColumnEntry {
    ColumnEntry {
        unique_name: unique_name.to_string(),
        label: lvl.to_string(),
        kind: "level".to_string(),
        hierarchy: Some(hierarchy.to_string()),
        level: Some(lvl.to_string()),
        semi_additive: None,
        required_dimension: None,
        is_calc: false,
        ..Default::default()
    }
}

fn enriched(entries: &[(&str, &[&str])]) -> (tempfile::NamedTempFile, EnrichedColumnGroups) {
    let columns: Vec<serde_json::Value> = entries
        .iter()
        .map(|(n, g)| serde_json::json!({ "unique_name": n, "column_group": g }))
        .collect();
    let cat = serde_json::json!({ "schema": "enriched-catalog.v1", "columns": columns });
    let mut f = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
    f.write_all(cat.to_string().as_bytes()).unwrap();
    let e = EnrichedColumnGroups::from_path(f.path()).unwrap();
    (f, e)
}

fn snapshot() -> CatalogSnapshot {
    CatalogSnapshot {
        columns: vec![
            measure("tpcds.inventory_quantity_on_hand"),
            measure("tpcds.total_store_sales"),
            level(
                "inventory_date_dimensions.[Inventory Calendar Month]",
                "inventory_date_dimensions",
                "Inventory Calendar Month",
            ),
            level(
                "sold_date_dimensions.[Sold Calendar Month]",
                "sold_date_dimensions",
                "Sold Calendar Month",
            ),
        ],
        ..CatalogSnapshot::default()
    }
}

fn tpcds_enriched() -> (tempfile::NamedTempFile, EnrichedColumnGroups) {
    enriched(&[
        ("tpcds.inventory_quantity_on_hand", &["inventory"]),
        ("tpcds.total_store_sales", &["store_sales"]),
        ("inventory_date_dimensions.[Inventory Calendar Month]", &["inventory"]),
        (
            "sold_date_dimensions.[Sold Calendar Month]",
            &["store_sales", "catalog_sales", "web_sales"],
        ),
    ])
}

/// AC: fm1-009 shape with BOTH per-measure date roles supplied → binds, each
/// measure carries its own date role (returns rows downstream).
#[test]
fn ac10_fm1_009_per_measure_roles_bind() {
    let (_f, e) = tpcds_enriched();
    let mqo = Mqo {
        model: "tpcds".to_string(),
        measures: vec![
            MeasureRef { unique_name: "tpcds.inventory_quantity_on_hand".to_string() },
            MeasureRef { unique_name: "tpcds.total_store_sales".to_string() },
        ],
        dimensions: vec![
            LevelSelection {
                hierarchy: "inventory_date_dimensions".to_string(),
                level: "Inventory Calendar Month".to_string(),
            },
            LevelSelection {
                hierarchy: "sold_date_dimensions".to_string(),
                level: "Sold Calendar Month".to_string(),
            },
        ],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: None,
        non_empty: false,
    };
    match bind_with_date_roles(&mqo, &snapshot(), &e) {
        BindResult::Bound(b) => {
            let inv = b.measures.iter().find(|m| m.unique_name.contains("inventory")).unwrap();
            let sales = b.measures.iter().find(|m| m.unique_name.contains("store_sales")).unwrap();
            assert_eq!(inv.date_role_hierarchy.as_deref(), Some("inventory_date_dimensions"));
            assert_eq!(sales.date_role_hierarchy.as_deref(), Some("sold_date_dimensions"));
        }
        other => panic!("expected Bound, got {other:?}"),
    }
}

/// AC2/AC3: the shared-single-Sold-Date trap (the `rejected` path in fm1-009) →
/// structured `cross_fact_date_incompatible` rejection naming the inventory measure.
#[test]
fn ac10_fm1_009_shared_sold_date_is_rejected() {
    let (_f, e) = tpcds_enriched();
    let mqo = Mqo {
        model: "tpcds".to_string(),
        measures: vec![
            MeasureRef { unique_name: "tpcds.inventory_quantity_on_hand".to_string() },
            MeasureRef { unique_name: "tpcds.total_store_sales".to_string() },
        ],
        dimensions: vec![LevelSelection {
            hierarchy: "sold_date_dimensions".to_string(),
            level: "Sold Calendar Month".to_string(),
        }],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: None,
        non_empty: false,
    };
    match bind_with_date_roles(&mqo, &snapshot(), &e) {
        BindResult::DateRoleIncompatible(rs) => {
            assert_eq!(rs.len(), 1);
            assert_eq!(rs[0].code, "cross_fact_date_incompatible");
            assert_eq!(rs[0].measure, "tpcds.inventory_quantity_on_hand");
        }
        other => panic!("expected DateRoleIncompatible, got {other:?}"),
    }
}

/// AC4 false-positive guard: a single-fact (sales-only) query under Sold month
/// must still bind — no false rejection.
#[test]
fn ac10_single_fact_sales_binds() {
    let (_f, e) = tpcds_enriched();
    let mqo = Mqo {
        model: "tpcds".to_string(),
        measures: vec![MeasureRef { unique_name: "tpcds.total_store_sales".to_string() }],
        dimensions: vec![LevelSelection {
            hierarchy: "sold_date_dimensions".to_string(),
            level: "Sold Calendar Month".to_string(),
        }],
        filters: vec![],
        time_intelligence: vec![],
        order: None,
        limit: None,
        non_empty: false,
    };
    assert!(matches!(
        bind_with_date_roles(&mqo, &snapshot(), &e),
        BindResult::Bound(_)
    ));
}
