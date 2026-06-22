//! AC6: Bounded results + error handling.
//!
//! - A fake source returning >1000 rows yields exactly 1000 + row_cap_tripped.
//! - Connection/auth failure → structured EngineError, no panic.

use std::{
    env,
    sync::{Arc, Mutex},
};

use mqo_auth_bridge::{
    executor::{EndpointConfig, LiveExecutor, RowSource},
    Backend, Engine, EngineError, OidcConfig, DEFAULT_MAX_RESULT_ROWS,
};
use serde_json::json;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

// ─── Fake row source that returns an oversized result ────────────────────────

struct OverflowRowSource {
    row_count: usize,
}

impl RowSource for OverflowRowSource {
    fn pgwire_query(
        &self,
        _host: &str,
        _port: u16,
        _pg_user: &str,
        _pg_pass: &str,
        _query: &str,
        _limit: usize,
        _deadline_secs: u64,
    ) -> Result<Vec<serde_json::Value>, EngineError> {
        let rows = (0..self.row_count).map(|i| json!({"n": i})).collect();
        Ok(rows)
    }

    fn xmla_query(
        &self,
        _xmla_url: &str,
        _bearer: &str,
        _query: &str,
        _catalog: &str,
        _cube: &str,
        _limit: usize,
        _deadline_secs: u64,
    ) -> Result<Vec<serde_json::Value>, EngineError> {
        let rows = (0..self.row_count).map(|i| json!({"n": i})).collect();
        Ok(rows)
    }

    fn xmla_discover(&self, _xmla_url: &str, _bearer: &str) -> Result<(), EngineError> {
        Ok(())
    }
}

// ─── Fake row source that always returns a connection error ──────────────────

struct ErrorRowSource;

impl RowSource for ErrorRowSource {
    fn pgwire_query(
        &self,
        _host: &str,
        _port: u16,
        _pg_user: &str,
        _pg_pass: &str,
        _query: &str,
        _limit: usize,
        _deadline_secs: u64,
    ) -> Result<Vec<serde_json::Value>, EngineError> {
        Err(EngineError::ConnectionFailure {
            reason: "simulated connection failure".to_string(),
        })
    }

    fn xmla_query(
        &self,
        _xmla_url: &str,
        _bearer: &str,
        _query: &str,
        _catalog: &str,
        _cube: &str,
        _limit: usize,
        _deadline_secs: u64,
    ) -> Result<Vec<serde_json::Value>, EngineError> {
        Err(EngineError::ConnectionFailure {
            reason: "simulated XMLA connection failure".to_string(),
        })
    }

    fn xmla_discover(&self, _xmla_url: &str, _bearer: &str) -> Result<(), EngineError> {
        Ok(())
    }
}

fn make_config(token_url: String, secret_var: &str) -> EndpointConfig {
    make_config_budget(token_url, secret_var, DEFAULT_MAX_RESULT_ROWS)
}

fn make_config_budget(token_url: String, secret_var: &str, budget: usize) -> EndpointConfig {
    EndpointConfig {
        pgwire_host: "localhost".to_string(),
        pgwire_port: 15432,
        xmla_url: "https://mcp-aws.atscaleinternal.com/v1/xmla".to_string(),
        oidc: OidcConfig {
            token_url,
            client_id: "test-client".to_string(),
            client_secret_env_var: secret_var.to_string(),
            realm: "test".to_string(),
            username: None,
            password_env_var: None,
        },
        pg_user: None,
        pg_pass: None,
        max_result_rows: budget,
        query_deadline_secs: mqo_auth_bridge::DEFAULT_QUERY_DEADLINE_SECS,
        query_deadline_max_secs: mqo_auth_bridge::DEFAULT_QUERY_DEADLINE_MAX_SECS,
        retry: Default::default(),
    }
}

/// Stand up a mock OIDC token endpoint that always returns a valid bearer token.
async fn token_mock() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "tok",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .mount(&server)
        .await;
    server
}

/// FR-1/FR-2 (G1): with the default budget, a result of N > 1000 rows persists
/// all N — proving the old hard-coded 1000-row clamp is gone. A 9,859-row source
/// (the `customers-ese-store-2001` shape) returns 9,859, not 1000.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn result_above_1000_under_budget_persists_full() {
    let server = token_mock().await;
    env::set_var("AC6_FULL_SECRET", "secret");
    let config = make_config(format!("{}/token", server.uri()), "AC6_FULL_SECRET");
    let exec =
        LiveExecutor::with_row_source(config, Arc::new(OverflowRowSource { row_count: 9859 }));

    let result = exec
        .execute("SELECT ...", Backend::Dax, None, Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"))
        .expect("should succeed");

    assert_eq!(
        result.rows.len(),
        9859,
        "full result must persist under the default budget ({DEFAULT_MAX_RESULT_ROWS})"
    );
    assert!(
        !result.row_cap_tripped,
        "row_cap_tripped must be false when the result is within budget"
    );
}

/// FR-3 (G3 / AC-3): a result that EXCEEDS the budget trips `row_cap_tripped`
/// and is truncated to exactly the budget — the signal the server turns into a
/// typed over-budget response, never a silent "complete" clamp.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn result_above_budget_trips_and_truncates_to_budget() {
    let server = token_mock().await;
    env::set_var("AC6_OVER_SECRET", "secret");
    // Budget 50_000; source returns 60_000 → over budget.
    let config = make_config(format!("{}/token", server.uri()), "AC6_OVER_SECRET");
    let exec =
        LiveExecutor::with_row_source(config, Arc::new(OverflowRowSource { row_count: 60_000 }));

    let result = exec
        .execute("SELECT ...", Backend::Dax, None, Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"))
        .expect("should succeed (clamped to budget)");

    assert_eq!(
        result.rows.len(),
        DEFAULT_MAX_RESULT_ROWS,
        "over-budget result must truncate to exactly the budget"
    );
    assert!(
        result.row_cap_tripped,
        "row_cap_tripped must be set when the result exceeds the budget"
    );
}

/// AC-4 (rollback): budget = 1000 reproduces today's behavior exactly — a
/// 2000-row source clamps to 1000 with the flag tripped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn budget_1000_reproduces_legacy_hard_cap() {
    let server = token_mock().await;
    env::set_var("AC6_LEGACY_SECRET", "secret");
    let config = make_config_budget(format!("{}/token", server.uri()), "AC6_LEGACY_SECRET", 1000);
    let exec =
        LiveExecutor::with_row_source(config, Arc::new(OverflowRowSource { row_count: 2000 }));

    let result = exec
        .execute("SELECT ...", Backend::Dax, None, Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"))
        .expect("should succeed (rows clamped)");

    assert_eq!(result.rows.len(), 1000, "budget=1000 must clamp to 1000");
    assert!(result.row_cap_tripped, "row_cap_tripped must be set at the legacy cap");
}

/// AC-5 (edge — exactly at budget): a result of exactly the budget persists in
/// full and does NOT trip.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn result_exactly_at_budget_does_not_trip() {
    let server = token_mock().await;
    env::set_var("AC6_EXACT_SECRET", "secret");
    let config = make_config_budget(format!("{}/token", server.uri()), "AC6_EXACT_SECRET", 1000);
    let exec =
        LiveExecutor::with_row_source(config, Arc::new(OverflowRowSource { row_count: 1000 }));

    let result = exec
        .execute("SELECT ...", Backend::Dax, None, Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"))
        .expect("should succeed");

    assert_eq!(result.rows.len(), 1000, "exactly-at-budget persists in full");
    assert!(
        !result.row_cap_tripped,
        "exactly-at-budget must NOT trip the over-budget signal"
    );
}

/// AC-6 (edge — zero rows): an empty result persists empty, no trip, no panic.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_row_result_persists_empty_no_trip() {
    let server = token_mock().await;
    env::set_var("AC6_ZERO_SECRET", "secret");
    let config = make_config(format!("{}/token", server.uri()), "AC6_ZERO_SECRET");
    let exec =
        LiveExecutor::with_row_source(config, Arc::new(OverflowRowSource { row_count: 0 }));

    let result = exec
        .execute("SELECT ...", Backend::Dax, None, Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"))
        .expect("should succeed");

    assert_eq!(result.rows.len(), 0, "empty result persists empty");
    assert!(!result.row_cap_tripped, "empty result must not trip");
}

/// AC-9 (regression — sub-1000): a small result is returned unchanged and never
/// trips, identical to pre-fix behavior.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sub_1000_result_unchanged() {
    let server = token_mock().await;
    env::set_var("AC6_SMALL_SECRET", "secret");
    let config = make_config(format!("{}/token", server.uri()), "AC6_SMALL_SECRET");
    let exec =
        LiveExecutor::with_row_source(config, Arc::new(OverflowRowSource { row_count: 42 }));

    let result = exec
        .execute("SELECT ...", Backend::Dax, None, Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"))
        .expect("should succeed");

    assert_eq!(result.rows.len(), 42, "small result returned unchanged");
    assert!(!result.row_cap_tripped, "small result must not trip");
}

/// A caller-supplied `limit` smaller than the result is an intentional bound
/// (top-N), not a truncation: rows are limited but `row_cap_tripped` stays
/// false (it means "exceeded the budget", not "exceeded the user limit").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_limit_bounds_without_tripping() {
    let server = token_mock().await;
    env::set_var("AC6_USERLIMIT_SECRET", "secret");
    let config = make_config(format!("{}/token", server.uri()), "AC6_USERLIMIT_SECRET");
    let exec =
        LiveExecutor::with_row_source(config, Arc::new(OverflowRowSource { row_count: 5000 }));

    let result = exec
        .execute("SELECT ...", Backend::Dax, Some(100), Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"))
        .expect("should succeed");

    assert_eq!(result.rows.len(), 100, "user limit bounds the rows");
    assert!(
        !result.row_cap_tripped,
        "a user limit is an intentional bound, not a budget trip"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_failure_is_structured_error_no_panic() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "tok",
            "expires_in": 3600,
            "token_type": "Bearer"
        })))
        .mount(&server)
        .await;

    env::set_var("AC6_ERR_SECRET", "secret");
    let config = make_config(format!("{}/token", server.uri()), "AC6_ERR_SECRET");
    let exec = LiveExecutor::with_row_source(config, Arc::new(ErrorRowSource));

    let err = exec
        .execute("SELECT ...", Backend::Dax, None, Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"))
        .expect_err("should return an error");

    // Must be structured, not a panic.
    match err {
        EngineError::ConnectionFailure { reason } => {
            assert!(reason.contains("simulated"), "wrong reason: {reason}");
        }
        other => panic!("expected ConnectionFailure, got: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_failure_missing_secret_is_structured_error() {
    let server = MockServer::start().await;
    // No mock registered — should never be reached.

    // Don't set AC6_MISSING_SECRET.
    env::remove_var("AC6_MISSING_SECRET");
    let config = make_config(format!("{}/token", server.uri()), "AC6_MISSING_SECRET");
    let exec = LiveExecutor::with_row_source(config, Arc::new(ErrorRowSource));

    let err = exec
        .execute("SELECT ...", Backend::Dax, None, Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"))
        .expect_err("should error on missing secret");

    match err {
        EngineError::MissingSecret { var_name } => {
            assert_eq!(var_name, "AC6_MISSING_SECRET");
        }
        other => panic!("expected MissingSecret, got: {other:?}"),
    }
}

// Silence unused Mutex import
#[allow(dead_code)]
fn _use_mutex(_: &Mutex<()>) {}
