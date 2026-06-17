//! Unit tests for `mqo-pg-query`.
//!
//! Tests that require a live AtScale cluster are marked `#[ignore]`.
//!
//! ## Manual smoke test (live cluster)
//!
//! ```bash
//! export ATSCALE_OIDC_SECRET=<client-secret>
//! cargo run -p mqo-pg-query -- \
//!   --sql "SELECT 1 AS n" \
//!   --pg-host mcp-aws.atscaleinternal.com \
//!   --pg-port 15432
//! ```
//!
//! For ROPC (direct user creds, qwf20-ground-truth-oracle path):
//! ```bash
//! export ATSCALE_OIDC_SECRET=<client-secret>
//! export ATSCALE_USER_PASS=<user-password>
//! cargo run -p mqo-pg-query -- \
//!   --sql "SELECT TOP 5 * FROM atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model" \
//!   --oidc-username joe.yen@atscale.com \
//!   --oidc-password-env ATSCALE_USER_PASS
//! ```

use mqo_pg_query::{
    config::QueryConfig,
    output::{QueryOutput, ReferenceTable},
    TypedCell, DEFAULT_MAX_ROWS, MAX_ROWS_CEILING,
};

// ─── TypedCell conversion ─────────────────────────────────────────────────────

#[test]
fn typed_cell_null_from_null_json() {
    let cell = TypedCell::from_value(serde_json::Value::Null);
    assert!(matches!(cell, TypedCell::Null));
}

#[test]
fn typed_cell_int_from_integer_json() {
    let cell = TypedCell::from_value(serde_json::json!(42_i64));
    assert!(matches!(cell, TypedCell::Int(42)));
}

#[test]
fn typed_cell_float_from_float_json() {
    let cell = TypedCell::from_value(serde_json::json!(3.14_f64));
    assert!(matches!(cell, TypedCell::Float(_)));
}

#[test]
fn typed_cell_text_from_string_json() {
    let cell = TypedCell::from_value(serde_json::json!("hello"));
    assert!(matches!(cell, TypedCell::Text(s) if s == "hello"));
}

#[test]
fn typed_cell_text_from_bool_json() {
    let cell = TypedCell::from_value(serde_json::json!(true));
    assert!(matches!(cell, TypedCell::Text(s) if s == "true"));
}

#[test]
fn typed_cell_negative_int() {
    let cell = TypedCell::from_value(serde_json::json!(-100_i64));
    assert!(matches!(cell, TypedCell::Int(-100)));
}

#[test]
fn typed_cell_large_positive_int() {
    let cell = TypedCell::from_value(serde_json::json!(1_000_000_i64));
    assert!(matches!(cell, TypedCell::Int(1_000_000)));
}

// ─── QueryOutput serialisation ────────────────────────────────────────────────

#[test]
fn query_output_empty_table_serialises_correctly() {
    let out = QueryOutput::Table(ReferenceTable {
        columns: vec![],
        rows: vec![],
    });
    let json = out.to_json();
    assert!(json.get("columns").is_some(), "must have columns key");
    assert!(json.get("rows").is_some(), "must have rows key");
    assert!(!json.get("columns").unwrap().as_array().unwrap().iter().next().is_some(),
        "columns must be empty");
}

#[test]
fn query_output_table_with_rows_serialises_correctly() {
    let out = QueryOutput::Table(ReferenceTable {
        columns: vec!["n".to_string(), "name".to_string()],
        rows: vec![
            vec![TypedCell::Int(1), TypedCell::Text("Alice".to_string())],
            vec![TypedCell::Int(2), TypedCell::Text("Bob".to_string())],
        ],
    });
    let json = out.to_json();
    let cols = json["columns"].as_array().unwrap();
    assert_eq!(cols.len(), 2);
    assert_eq!(cols[0].as_str().unwrap(), "n");
    assert_eq!(cols[1].as_str().unwrap(), "name");
    let rows = json["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0].as_i64().unwrap(), 1);
    assert_eq!(rows[0][1].as_str().unwrap(), "Alice");
}

#[test]
fn query_output_oversize_serialises_correctly() {
    let out = QueryOutput::oversize(50_001, 50_000);
    let json = out.to_json();
    assert!(json.get("oversize").is_some(), "must have 'oversize' key");
    let oversize = &json["oversize"];
    assert_eq!(oversize["observed_at_least"].as_u64().unwrap(), 50_001);
    assert_eq!(oversize["cap"].as_u64().unwrap(), 50_000);
    // Must NOT have columns/rows/error at top level.
    assert!(json.get("columns").is_none());
    assert!(json.get("error").is_none());
}

#[test]
fn query_output_error_serialises_correctly() {
    let out = QueryOutput::error("OIDC authentication failed (see --oidc-token-url)");
    let json = out.to_json();
    assert!(json.get("error").is_some(), "must have 'error' key");
    let err = &json["error"];
    assert!(err.get("message").is_some(), "must have 'message' key");
    // Must NOT have columns/rows/oversize at top level.
    assert!(json.get("columns").is_none());
    assert!(json.get("oversize").is_none());
}

#[test]
fn query_output_error_no_secret_in_message() {
    // AC5: no credentials appear in output.
    let out = QueryOutput::error("OIDC authentication failed (see --oidc-token-url)");
    let json = out.to_json().to_string();
    // Should not contain token-like substrings (this is a unit-level check;
    // the integration check uses grep on real output).
    assert!(!json.contains("secret"), "secret must not appear in error output");
    assert!(!json.contains("password"), "password must not appear in error output");
    assert!(!json.contains("bearer"), "bearer must not appear in error output");
}

#[test]
fn query_output_is_success_for_table() {
    let out = QueryOutput::Table(ReferenceTable { columns: vec![], rows: vec![] });
    assert!(out.is_success());
}

#[test]
fn query_output_is_success_for_oversize() {
    let out = QueryOutput::oversize(1, 1);
    assert!(out.is_success());
}

#[test]
fn query_output_is_not_success_for_error() {
    let out = QueryOutput::error("something went wrong");
    assert!(!out.is_success());
}

// ─── Row cap constants ────────────────────────────────────────────────────────

#[test]
fn default_max_rows_is_50k() {
    assert_eq!(DEFAULT_MAX_ROWS, 50_000);
}

#[test]
fn max_rows_ceiling_is_200k() {
    assert_eq!(MAX_ROWS_CEILING, 200_000);
}

// ─── QueryConfig defaults ─────────────────────────────────────────────────────

#[test]
fn query_config_default_values_are_sane() {
    let cfg = QueryConfig {
        pg_host: "mcp-aws.atscaleinternal.com".to_string(),
        pg_port: 15432,
        oidc_token_url: "https://example.com/token".to_string(),
        oidc_client_id: "atscale-mcp".to_string(),
        oidc_client_secret_env: "ATSCALE_OIDC_SECRET".to_string(),
        oidc_realm: "atscale".to_string(),
        oidc_username: None,
        oidc_password_env: None,
        pg_user: None,
        pg_pass_resolved: None,
        max_result_rows: DEFAULT_MAX_ROWS,
        timeout_secs: 120,
    };
    assert_eq!(cfg.pg_port, 15432);
    assert_eq!(cfg.max_result_rows, 50_000);
    assert_eq!(cfg.timeout_secs, 120);
    assert!(cfg.pg_user.is_none());
    assert!(cfg.pg_pass_resolved.is_none());
    // Secret env var name present, not a value.
    assert_eq!(cfg.oidc_client_secret_env, "ATSCALE_OIDC_SECRET");
}

// ─── run_query: empty SQL error ───────────────────────────────────────────────

#[test]
fn run_query_rejects_empty_sql() {
    let cfg = QueryConfig {
        pg_host: "localhost".to_string(),
        pg_port: 15432,
        oidc_token_url: "https://example.com/token".to_string(),
        oidc_client_id: "test".to_string(),
        oidc_client_secret_env: "NONEXISTENT_SECRET_ENV".to_string(),
        oidc_realm: "test".to_string(),
        oidc_username: None,
        oidc_password_env: None,
        pg_user: None,
        pg_pass_resolved: None,
        max_result_rows: DEFAULT_MAX_ROWS,
        timeout_secs: 120,
    };
    let result = mqo_pg_query::run_query(&cfg, "");
    assert!(result.is_err(), "empty SQL should return an error");
    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("empty"), "error message should mention empty SQL");
}

#[test]
fn run_query_rejects_whitespace_only_sql() {
    let cfg = QueryConfig {
        pg_host: "localhost".to_string(),
        pg_port: 15432,
        oidc_token_url: "https://example.com/token".to_string(),
        oidc_client_id: "test".to_string(),
        oidc_client_secret_env: "NONEXISTENT_SECRET_ENV".to_string(),
        oidc_realm: "test".to_string(),
        oidc_username: None,
        oidc_password_env: None,
        pg_user: None,
        pg_pass_resolved: None,
        max_result_rows: DEFAULT_MAX_ROWS,
        timeout_secs: 120,
    };
    let result = mqo_pg_query::run_query(&cfg, "   \t\n  ");
    assert!(result.is_err(), "whitespace-only SQL should return an error");
}

// ─── Live cluster tests (require ATSCALE_OIDC_SECRET) ────────────────────────
//
// Run manually with:
//   export ATSCALE_OIDC_SECRET=<secret>
//   cargo test -p mqo-pg-query -- --ignored
//
// Or the full smoke test binary:
//   cargo run -p mqo-pg-query -- --sql "SELECT 1 AS n"

/// Live test: SELECT 1 returns a single-row table (AC1 / AC4 proxy).
///
/// # Ignored
/// Requires a live AtScale PGWire endpoint and `ATSCALE_OIDC_SECRET`.
#[test]
#[ignore = "requires live AtScale cluster and ATSCALE_OIDC_SECRET env var"]
fn live_select_1_returns_row() {
    let cfg = QueryConfig {
        pg_host: "mcp-aws.atscaleinternal.com".to_string(),
        pg_port: 15432,
        oidc_token_url:
            "https://mcp-aws.atscaleinternal.com/auth/realms/atscale/protocol/openid-connect/token"
                .to_string(),
        oidc_client_id: "atscale-mcp".to_string(),
        oidc_client_secret_env: "ATSCALE_OIDC_SECRET".to_string(),
        oidc_realm: "atscale".to_string(),
        oidc_username: None,
        oidc_password_env: None,
        pg_user: None,
        pg_pass_resolved: None,
        max_result_rows: DEFAULT_MAX_ROWS,
        timeout_secs: 120,
    };
    let result = mqo_pg_query::run_query(&cfg, "SELECT 1 AS n");
    assert!(result.is_ok(), "live SELECT 1 should succeed: {:?}", result.err());
    let output = result.unwrap();
    assert!(output.is_success());
    let json = output.to_json();
    assert!(json.get("columns").is_some(), "should have columns");
    assert!(json.get("rows").is_some(), "should have rows");
}

/// Live test: bad credential emits structured JSON error, exits non-zero (AC3).
///
/// # Ignored
/// Requires a live AtScale PGWire endpoint. Does NOT require a valid secret.
#[test]
#[ignore = "requires live AtScale cluster (no valid credential needed)"]
fn live_bad_credential_returns_error_no_secret() {
    // Use an obviously-invalid secret so OIDC auth fails.
    // Set the env var to a junk value.
    std::env::set_var("MQO_PG_QUERY_TEST_BAD_SECRET", "invalid_secret_value");
    let cfg = QueryConfig {
        pg_host: "mcp-aws.atscaleinternal.com".to_string(),
        pg_port: 15432,
        oidc_token_url:
            "https://mcp-aws.atscaleinternal.com/auth/realms/atscale/protocol/openid-connect/token"
                .to_string(),
        oidc_client_id: "atscale-mcp".to_string(),
        oidc_client_secret_env: "MQO_PG_QUERY_TEST_BAD_SECRET".to_string(),
        oidc_realm: "atscale".to_string(),
        oidc_username: None,
        oidc_password_env: None,
        pg_user: None,
        pg_pass_resolved: None,
        max_result_rows: DEFAULT_MAX_ROWS,
        timeout_secs: 30,
    };
    let result = mqo_pg_query::run_query(&cfg, "SELECT 1 AS n");
    assert!(result.is_err(), "bad credential should produce an error");
    let err = result.unwrap_err().to_string();
    // Must not contain the literal secret value.
    assert!(
        !err.contains("invalid_secret_value"),
        "error must not leak the secret value: {err}"
    );
}
