#![allow(clippy::expect_used, clippy::missing_const_for_fn)]
//! AC1: Given a catalog and bindings where measure M is on fact F,
//! when enrich runs, then M's `column_group` set equals {F} exactly.

use mqoguard_column_group_enrichment::{enrich, CatalogColumn, CatalogSnapshot, FactBindings};
use std::collections::{BTreeMap, BTreeSet};

fn single_measure_catalog(unique_name: &str) -> CatalogSnapshot {
    CatalogSnapshot {
        columns: vec![CatalogColumn {
            unique_name: unique_name.to_string(),
            kind: Some("measure".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn bindings_with_measure(unique_name: &str, group: &str) -> FactBindings {
    let mut measures = BTreeMap::new();
    measures.insert(
        unique_name.to_string(),
        std::iter::once(group.to_string()).collect::<BTreeSet<_>>(),
    );
    FactBindings {
        measures,
        hierarchies: BTreeMap::new(),
    }
}

fn single_group(name: &str) -> BTreeSet<String> {
    std::iter::once(name.to_string()).collect()
}

#[test]
fn ac1_measure_bound_to_single_fact() {
    let catalog = single_measure_catalog("model.my_measure");
    let bindings = bindings_with_measure("model.my_measure", "store_sales");
    let enriched = enrich(&catalog, &bindings);

    assert_eq!(enriched.columns.len(), 1);
    let col = enriched.columns.first().expect("one column");
    let expected = single_group("store_sales");
    assert_eq!(col.column_group, expected, "AC1: measure column_group must equal {{F}} exactly");
}

#[test]
fn ac1_inventory_measure_bound_correctly() {
    let catalog = single_measure_catalog("tpcds_benchmark_model.inventory_quantity_on_hand");
    let bindings = FactBindings::tpcds_defaults();
    let enriched = enrich(&catalog, &bindings);

    let col = enriched.columns.first().expect("one column");
    let expected = single_group("inventory");
    assert_eq!(
        col.column_group, expected,
        "AC1: inventory_quantity_on_hand must belong to inventory fact only"
    );
}

#[test]
fn ac1_catalog_sales_measure_bound_correctly() {
    let catalog = single_measure_catalog("tpcds_benchmark_model.catalog_quantity_sold");
    let bindings = FactBindings::tpcds_defaults();
    let enriched = enrich(&catalog, &bindings);

    let col = enriched.columns.first().expect("one column");
    let expected = single_group("catalog_sales");
    assert_eq!(
        col.column_group, expected,
        "AC1: catalog_quantity_sold must belong to catalog_sales only"
    );
}

#[test]
fn ac1_web_measure_bound_correctly() {
    let catalog = single_measure_catalog("tpcds_benchmark_model.web_quantity_sold");
    let bindings = FactBindings::tpcds_defaults();
    let enriched = enrich(&catalog, &bindings);

    let col = enriched.columns.first().expect("one column");
    let expected = single_group("web_sales");
    assert_eq!(col.column_group, expected, "AC1: web measure");
}
