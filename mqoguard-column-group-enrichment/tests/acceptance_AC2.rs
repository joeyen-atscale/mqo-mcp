#![allow(clippy::expect_used, clippy::missing_const_for_fn)]
//! AC2: Given a dimension level L joinable to facts F1 and F2,
//! when enrich runs, then L's `column_group` set equals {F1, F2}.

use mqoguard_column_group_enrichment::{enrich, CatalogColumn, CatalogSnapshot, FactBindings};
use std::collections::{BTreeMap, BTreeSet};

fn level_catalog(unique_name: &str, hierarchy: &str) -> CatalogSnapshot {
    CatalogSnapshot {
        columns: vec![CatalogColumn {
            unique_name: unique_name.to_string(),
            kind: Some("level".to_string()),
            hierarchy: Some(hierarchy.to_string()),
            level: Some("Some Level".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn bindings_with_hierarchy(hierarchy: &str, groups: &[&str]) -> FactBindings {
    let mut hierarchies = BTreeMap::new();
    hierarchies.insert(
        hierarchy.to_string(),
        groups.iter().map(ToString::to_string).collect::<BTreeSet<_>>(),
    );
    FactBindings {
        measures: BTreeMap::new(),
        hierarchies,
    }
}

fn groups_set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(ToString::to_string).collect()
}

#[test]
fn ac2_conformed_dimension_gets_all_facts() {
    let catalog = level_catalog("h.product.[Product Name]", "h.product");
    let bindings = bindings_with_hierarchy("h.product", &["store_sales", "web_sales"]);
    let enriched = enrich(&catalog, &bindings);

    let col = enriched.columns.first().expect("one column");
    let expected = groups_set(&["store_sales", "web_sales"]);
    assert_eq!(
        col.column_group, expected,
        "AC2: conformed dimension must carry all N fact groups"
    );
}

#[test]
fn ac2_tpcds_product_dimension_is_conformed() {
    // product_dimension is bound to all 7 TPC-DS facts
    let catalog = level_catalog(
        "product_dimension.[Item Description]",
        "product_dimension",
    );
    let bindings = FactBindings::tpcds_defaults();
    let enriched = enrich(&catalog, &bindings);

    let col = enriched.columns.first().expect("one column");
    let groups = &col.column_group;
    assert!(groups.contains("store_sales"), "AC2: product_dimension must include store_sales");
    assert!(groups.contains("inventory"), "AC2: product_dimension must include inventory");
    assert!(
        groups.contains("catalog_sales"),
        "AC2: product_dimension must include catalog_sales"
    );
    assert!(groups.contains("web_sales"), "AC2: product_dimension must include web_sales");
    assert!(
        groups.len() >= 4,
        "AC2: product_dimension must span at least 4 facts, got {}",
        groups.len()
    );
}

#[test]
fn ac2_customer_dimension_conformed_across_sales_and_returns() {
    let catalog = level_catalog("customer_dimension.[Customer ID]", "customer_dimension");
    let bindings = FactBindings::tpcds_defaults();
    let enriched = enrich(&catalog, &bindings);

    let col = enriched.columns.first().expect("one column");
    let groups = &col.column_group;
    assert!(groups.contains("store_sales"));
    assert!(groups.contains("store_returns"));
    assert!(groups.contains("catalog_sales"));
    assert!(groups.contains("web_sales"));
}

#[test]
fn ac2_three_fact_conformed_dimension() {
    let catalog = level_catalog("dim.date.[Date]", "dim.date");
    let bindings = bindings_with_hierarchy("dim.date", &["fact_a", "fact_b", "fact_c"]);
    let enriched = enrich(&catalog, &bindings);

    let col = enriched.columns.first().expect("one column");
    let expected = groups_set(&["fact_a", "fact_b", "fact_c"]);
    assert_eq!(col.column_group, expected);
}
