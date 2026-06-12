//! Parse failure tests: invalid SQL and unknown column names.

use mqo_catalog_binder::catalog::CatalogSnapshot;
use mqo_from_sql_lib::{compile_sql_with_snapshot, parser::parse_sql};

fn load_snapshot() -> CatalogSnapshot {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/catalog_snapshot.json"
    );
    let text = std::fs::read_to_string(path).expect("fixture catalog snapshot");
    serde_json::from_str(&text).expect("valid catalog snapshot JSON")
}

// ── Parse-level failures ──────────────────────────────────────────────────────

#[test]
fn invalid_sql_returns_parse_error() {
    let result = parse_sql("THIS IS NOT SQL");
    assert!(result.is_err(), "invalid SQL should produce an error");
    let err = result.unwrap_err().to_string();
    // Error message should not be empty
    assert!(!err.is_empty(), "error message should be non-empty");
}

#[test]
fn wildcard_select_returns_error() {
    let result = parse_sql(r#"SELECT * FROM "cat"."model""#);
    assert!(result.is_err(), "wildcard SELECT should be rejected");
}

#[test]
fn empty_sql_returns_error() {
    let result = parse_sql("");
    assert!(result.is_err(), "empty SQL should return an error");
}

#[test]
fn non_select_statement_returns_error() {
    // INSERT is not a valid AtScale projection
    let result = parse_sql(r#"INSERT INTO "foo" VALUES (1)"#);
    assert!(result.is_err(), "INSERT should be rejected");
}

// ── Resolve-level failures ────────────────────────────────────────────────────

#[test]
fn unknown_measure_returns_descriptive_error() {
    let snapshot = load_snapshot();
    let sql = r#"SELECT "FakeColumnThatDoesNotExist" FROM "atscale_catalogs"."cat"."model""#;

    let result = compile_sql_with_snapshot(sql, &snapshot);
    assert!(result.is_err(), "unknown measure should fail");
    let err = result.unwrap_err().to_string();
    // The error message must contain the offending fragment
    assert!(
        err.contains("FakeColumnThatDoesNotExist"),
        "error should mention the bad column: {err}"
    );
}

#[test]
fn unknown_dimension_returns_descriptive_error() {
    let snapshot = load_snapshot();
    // The SELECT has a valid measure; GROUP BY has an unknown dimension
    let sql = r#"
        SELECT "store_sales.Total Store Sales", "UnknownDimension.fake.[Lvl]"
        FROM "atscale_catalogs"."tpcds_Snowflake"."tpcds_model"
        GROUP BY "UnknownDimension.fake.[Lvl]"
    "#;

    let result = compile_sql_with_snapshot(sql, &snapshot);
    assert!(result.is_err(), "unknown dimension should fail");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("UnknownDimension.fake.[Lvl]"),
        "error should mention the bad column: {err}"
    );
}

#[test]
fn parse_error_message_not_empty() {
    let result = parse_sql("SELECT FROM WHERE GROUP");
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(!err.is_empty());
}
