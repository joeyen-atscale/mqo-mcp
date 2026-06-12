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
    Backend, Engine, EngineError, OidcConfig, HARD_ROW_CAP,
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
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_source_yields_exactly_hard_cap_rows() {
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

    env::set_var("AC6_OIDC_SECRET", "secret");
    let config = make_config(format!("{}/token", server.uri()), "AC6_OIDC_SECRET");
    let exec =
        LiveExecutor::with_row_source(config, Arc::new(OverflowRowSource { row_count: 2000 }));

    let result = exec
        .execute("SELECT ...", Backend::Dax, None, Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"))
        .expect("should succeed (rows clamped)");

    assert_eq!(
        result.rows.len(),
        HARD_ROW_CAP,
        "must clamp to HARD_ROW_CAP={HARD_ROW_CAP}"
    );
    assert!(
        result.row_cap_tripped,
        "row_cap_tripped must be set when result was clamped"
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
