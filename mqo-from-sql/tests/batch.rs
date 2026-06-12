//! Batch mode tests: JSONL input with mixed success and error lines.

use mqo_catalog_binder::catalog::CatalogSnapshot;
use mqo_from_sql_lib::compile_sql_with_snapshot;

fn load_snapshot() -> CatalogSnapshot {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/catalog_snapshot.json"
    );
    let text = std::fs::read_to_string(path).expect("fixture catalog snapshot");
    serde_json::from_str(&text).expect("valid catalog snapshot JSON")
}

/// Simulate batch processing: parse each line and collect results.
fn process_batch(
    lines: &[&str],
    snapshot: &CatalogSnapshot,
) -> Vec<Result<mqo_spec::BoundMqo, String>> {
    lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            compile_sql_with_snapshot(line.trim(), snapshot)
                .map_err(|e| e.to_string())
        })
        .collect()
}

#[test]
fn batch_five_lines_one_bad() {
    let snapshot = load_snapshot();

    let lines = [
        // Line 1 — good: simple measure
        r#"SELECT "store_sales.Total Store Sales" FROM "atscale_catalogs"."tpcds_Snowflake"."tpcds_model""#,
        // Line 2 — good: measure + dimension
        r#"SELECT "store_sales.Number of Store Sales", "time_dim.calendar.[Year]" FROM "atscale_catalogs"."tpcds_Snowflake"."model" GROUP BY "time_dim.calendar.[Year]""#,
        // Line 3 — good: measure + limit
        r#"SELECT "store_sales.Total Store Sales" FROM "atscale_catalogs"."tpcds_Snowflake"."tpcds_model" LIMIT 10"#,
        // Line 4 — bad: unknown measure name
        r#"SELECT "NonExistentMeasureXYZ" FROM "atscale_catalogs"."tpcds_Snowflake"."tpcds_model""#,
        // Line 5 — good: measure + two dimensions
        r#"SELECT "store_sales.Store Sales Net Profit", "time_dim.calendar.[Year]", "customer.geography.[State]" FROM "atscale_catalogs"."tpcds_Snowflake"."model" GROUP BY "time_dim.calendar.[Year]", "customer.geography.[State]""#,
    ];

    let results = process_batch(&lines, &snapshot);

    assert_eq!(results.len(), 5, "should process exactly 5 lines");

    // Lines 0, 1, 2, 4 should succeed
    assert!(results[0].is_ok(), "line 1 should succeed");
    assert!(results[1].is_ok(), "line 2 should succeed");
    assert!(results[2].is_ok(), "line 3 should succeed");
    assert!(results[4].is_ok(), "line 5 should succeed");

    // Line 3 (index 3) should fail with a descriptive error
    let bad = results[3].as_ref().unwrap_err();
    assert!(
        bad.contains("NonExistentMeasureXYZ"),
        "error should mention the bad column name: {bad}"
    );

    // Count failures
    let failures: Vec<_> = results.iter().filter(|r| r.is_err()).collect();
    assert_eq!(failures.len(), 1, "exactly 1 line should fail");
}

#[test]
fn batch_error_mentions_column_name() {
    let snapshot = load_snapshot();
    let bad_sql = r#"SELECT "ImaginaryMeasure123" FROM "cat"."model""#;

    let result = compile_sql_with_snapshot(bad_sql, &snapshot);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("ImaginaryMeasure123"),
        "error message should contain the unknown column name: {err_msg}"
    );
}

#[test]
fn batch_all_good_lines_produce_bound_mqo() {
    let snapshot = load_snapshot();
    let good_lines = [
        r#"SELECT "store_sales.Total Store Sales" FROM "atscale_catalogs"."tpcds_Snowflake"."m""#,
        r#"SELECT "store_sales.Number of Store Sales" FROM "atscale_catalogs"."tpcds_Snowflake"."m" LIMIT 5"#,
    ];

    let results = process_batch(&good_lines, &snapshot);
    for (i, r) in results.iter().enumerate() {
        assert!(r.is_ok(), "line {i} should be ok, got: {:?}", r.as_ref().err());
    }
}
