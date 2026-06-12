//! Roundtrip tests: SQL → parse → resolve → BoundMqo assertions.
//!
//! Each case supplies a SQL string (the kind `build_sql_projection` would emit)
//! and asserts the resulting BoundMqo contains the expected measures/dimensions.

use mqo_catalog_binder::catalog::CatalogSnapshot;
use mqo_from_sql_lib::{compile_sql_with_snapshot};

fn load_snapshot() -> CatalogSnapshot {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/catalog_snapshot.json"
    );
    let text = std::fs::read_to_string(path).expect("fixture catalog snapshot");
    serde_json::from_str(&text).expect("valid catalog snapshot JSON")
}

// ── Case 1: simple measure + dimension ────────────────────────────────────────

#[test]
fn roundtrip_measure_and_dimension() {
    let snapshot = load_snapshot();
    let sql = r#"
        SELECT "store_sales.Total Store Sales", "time_dim.calendar.[Year]"
        FROM "atscale_catalogs"."tpcds_Snowflake"."tpcds_model"
        GROUP BY "time_dim.calendar.[Year]"
    "#;

    let bound = compile_sql_with_snapshot(sql, &snapshot).expect("roundtrip should succeed");

    // Should have exactly 1 measure
    assert_eq!(bound.measures.len(), 1);
    assert_eq!(
        bound.measures[0].unique_name,
        "store_sales.Total Store Sales"
    );

    // Should have exactly 1 dimension
    assert_eq!(bound.dimensions.len(), 1);
    assert_eq!(bound.dimensions[0].unique_name, "time_dim.calendar.[Year]");
    assert_eq!(bound.dimensions[0].hierarchy, "time_dim.calendar");

    // MQO model should be the last FROM part
    assert_eq!(bound.mqo.model, "tpcds_model");
    assert!(bound.mqo.limit.is_none());
}

// ── Case 2: filter ────────────────────────────────────────────────────────────

#[test]
fn roundtrip_with_filter() {
    let snapshot = load_snapshot();
    let sql = r#"
        SELECT "store_sales.Number of Store Sales", "time_dim.calendar.[Year]"
        FROM "atscale_catalogs"."tpcds_Snowflake"."tpcds_model"
        WHERE "time_dim.calendar.[Year]" = '2022'
        GROUP BY "time_dim.calendar.[Year]"
    "#;

    let bound = compile_sql_with_snapshot(sql, &snapshot).expect("roundtrip with filter");

    assert_eq!(bound.measures.len(), 1);
    assert_eq!(bound.measures[0].unique_name, "store_sales.Number of Store Sales");
    assert_eq!(bound.dimensions.len(), 1);

    // Filter should have been built
    assert!(!bound.mqo.filters.is_empty(), "expected at least one filter");
}

// ── Case 3: limit ─────────────────────────────────────────────────────────────

#[test]
fn roundtrip_with_limit() {
    let snapshot = load_snapshot();
    let sql = r#"
        SELECT "store_sales.Total Store Sales", "item.category.[Category]"
        FROM "atscale_catalogs"."tpcds_Snowflake"."tpcds_model"
        GROUP BY "item.category.[Category]"
        LIMIT 50
    "#;

    let bound = compile_sql_with_snapshot(sql, &snapshot).expect("roundtrip with limit");

    assert_eq!(bound.mqo.limit, Some(50));
    assert_eq!(bound.measures.len(), 1);
    assert_eq!(bound.dimensions.len(), 1);
    assert_eq!(bound.dimensions[0].unique_name, "item.category.[Category]");
}

// ── Case 4: multiple measures ─────────────────────────────────────────────────

#[test]
fn roundtrip_multiple_measures() {
    let snapshot = load_snapshot();
    let sql = r#"
        SELECT "store_sales.Total Store Sales", "store_sales.Store Sales Net Profit"
        FROM "atscale_catalogs"."tpcds_Snowflake"."tpcds_model"
    "#;

    let bound = compile_sql_with_snapshot(sql, &snapshot).expect("multiple measures");

    assert_eq!(bound.measures.len(), 2);
    let names: Vec<&str> = bound.measures.iter().map(|m| m.unique_name.as_str()).collect();
    assert!(names.contains(&"store_sales.Total Store Sales"));
    assert!(names.contains(&"store_sales.Store Sales Net Profit"));
    assert!(bound.dimensions.is_empty());
}

// ── Case 5: measure + multiple dimensions ─────────────────────────────────────

#[test]
fn roundtrip_multiple_dimensions() {
    let snapshot = load_snapshot();
    let sql = r#"
        SELECT "store_sales.Total Store Sales",
               "time_dim.calendar.[Year]",
               "customer.geography.[State]"
        FROM "atscale_catalogs"."tpcds_Snowflake"."tpcds_model"
        GROUP BY "time_dim.calendar.[Year]", "customer.geography.[State]"
    "#;

    let bound = compile_sql_with_snapshot(sql, &snapshot).expect("multiple dimensions");

    assert_eq!(bound.measures.len(), 1);
    assert_eq!(bound.dimensions.len(), 2);
    let dim_names: Vec<&str> = bound.dimensions.iter().map(|d| d.unique_name.as_str()).collect();
    assert!(dim_names.contains(&"time_dim.calendar.[Year]"));
    assert!(dim_names.contains(&"customer.geography.[State]"));
}
