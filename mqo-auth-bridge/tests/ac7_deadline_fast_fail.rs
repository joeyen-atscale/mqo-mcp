//! AC7: Execution deadline fast-fail tests.
//!
//! Verifies that:
//! - A fake [`RowSource`] that returns `QueryDeadlineExceeded` is surfaced
//!   correctly by [`LiveExecutor`] (AC1, FR6).
//! - The `resolve_deadline` helper clamps per-request overrides to the
//!   configured max (AC6, FR5).
//! - An unparseable deadline override is clamped to the default (NFR2).
//! - The `QueryDeadlineExceeded` error carries `elapsed_secs`,
//!   `deadline_secs`, and a non-empty `hint` (FR6, G2).
//! - Queries that complete under the deadline return their rows unchanged
//!   (AC2, guardrail).
//!
//! All tests use a fake [`RowSource`] — no live cluster required.

use std::{
    env,
    sync::{Arc, Mutex},
};

use mqo_auth_bridge::{
    executor::{EndpointConfig, LiveExecutor, RowSource},
    Backend, Engine, EngineError, OidcConfig, DEFAULT_MAX_RESULT_ROWS,
    DEFAULT_QUERY_DEADLINE_MAX_SECS, DEFAULT_QUERY_DEADLINE_SECS,
};
use serde_json::json;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

// ─── Fake RowSource that simulates a deadline breach ─────────────────────────

/// Returns `QueryDeadlineExceeded` for every query (simulates a slow backend).
struct SlowRowSource;

impl RowSource for SlowRowSource {
    fn pgwire_query(
        &self,
        _host: &str,
        _port: u16,
        _pg_user: &str,
        _pg_pass: &str,
        _query: &str,
        _limit: usize,
        deadline_secs: u64,
    ) -> Result<Vec<serde_json::Value>, EngineError> {
        Err(EngineError::QueryDeadlineExceeded {
            elapsed_secs: deadline_secs,
            deadline_secs,
            hint: mqo_auth_bridge::DEADLINE_EXCEEDED_HINT.to_string(),
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
        deadline_secs: u64,
    ) -> Result<Vec<serde_json::Value>, EngineError> {
        Err(EngineError::QueryDeadlineExceeded {
            elapsed_secs: deadline_secs,
            deadline_secs,
            hint: mqo_auth_bridge::DEADLINE_EXCEEDED_HINT.to_string(),
        })
    }

    fn xmla_discover(&self, _xmla_url: &str, _bearer: &str) -> Result<(), EngineError> {
        Ok(())
    }
}

// ─── Fake RowSource that always succeeds with one row ────────────────────────

/// Returns a single synthetic row (simulates a fast query under the deadline).
struct FastRowSource;

impl RowSource for FastRowSource {
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
        Ok(vec![json!({"col": "fast-pgwire"})])
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
        Ok(vec![json!({"col": "fast-xmla"})])
    }

    fn xmla_discover(&self, _xmla_url: &str, _bearer: &str) -> Result<(), EngineError> {
        Ok(())
    }
}

// ─── Deadline-aware fake that records the deadline it was called with ─────────

#[derive(Default)]
struct DeadlineCapturingSource {
    captured_pgwire_deadline: Mutex<Option<u64>>,
    captured_xmla_deadline: Mutex<Option<u64>>,
}

impl RowSource for DeadlineCapturingSource {
    fn pgwire_query(
        &self,
        _host: &str,
        _port: u16,
        _pg_user: &str,
        _pg_pass: &str,
        _query: &str,
        _limit: usize,
        deadline_secs: u64,
    ) -> Result<Vec<serde_json::Value>, EngineError> {
        *self.captured_pgwire_deadline.lock().unwrap() = Some(deadline_secs);
        Ok(vec![json!({"col": "ok"})])
    }

    fn xmla_query(
        &self,
        _xmla_url: &str,
        _bearer: &str,
        _query: &str,
        _catalog: &str,
        _cube: &str,
        _limit: usize,
        deadline_secs: u64,
    ) -> Result<Vec<serde_json::Value>, EngineError> {
        *self.captured_xmla_deadline.lock().unwrap() = Some(deadline_secs);
        Ok(vec![json!({"col": "ok"})])
    }

    fn xmla_discover(&self, _xmla_url: &str, _bearer: &str) -> Result<(), EngineError> {
        Ok(())
    }
}

// ─── Test helpers ─────────────────────────────────────────────────────────────

async fn token_stub() -> MockServer {
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

fn make_config(token_url: String, secret_var: &str, deadline_secs: u64) -> EndpointConfig {
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
        max_result_rows: DEFAULT_MAX_RESULT_ROWS,
        query_deadline_secs: deadline_secs,
        query_deadline_max_secs: DEFAULT_QUERY_DEADLINE_MAX_SECS,
        retry: Default::default(),
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

/// AC1 (FR1, G2): A slow backend that returns `QueryDeadlineExceeded` on the
/// PGWire path is surfaced correctly to the caller with the correct variant and
/// all required fields populated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pgwire_deadline_exceeded_is_surfaced() {
    env::set_var("AC7_PG_SECRET", "secret");
    let server = token_stub().await;
    let config = make_config(format!("{}/token", server.uri()), "AC7_PG_SECRET", 5);
    let exec = LiveExecutor::with_row_source(config, Arc::new(SlowRowSource));

    let err = exec
        .execute("SELECT slow_query", Backend::Sql, None, None)
        .expect_err("expected QueryDeadlineExceeded");

    match err {
        EngineError::QueryDeadlineExceeded {
            elapsed_secs,
            deadline_secs,
            ref hint,
        } => {
            assert_eq!(deadline_secs, 5, "deadline_secs should match config");
            assert_eq!(elapsed_secs, deadline_secs, "elapsed_secs should be set");
            assert!(!hint.is_empty(), "hint must be non-empty (FR6)");
            assert!(
                hint.contains("deadline"),
                "hint should mention 'deadline'; got: {hint}"
            );
        }
        other => panic!("expected QueryDeadlineExceeded, got: {other:?}"),
    }
}

/// AC1 (FR3, G2): A slow backend that returns `QueryDeadlineExceeded` on the
/// XMLA path (DAX/MDX) is surfaced correctly with a populated `hint`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn xmla_deadline_exceeded_is_surfaced() {
    env::set_var("AC7_XMLA_SECRET", "secret");
    let server = token_stub().await;
    let config = make_config(format!("{}/token", server.uri()), "AC7_XMLA_SECRET", 10);
    let exec = LiveExecutor::with_row_source(config, Arc::new(SlowRowSource));

    let err = exec
        .execute(
            "EVALUATE ROW(\"x\", 1)",
            Backend::Dax,
            None,
            Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"),
        )
        .expect_err("expected QueryDeadlineExceeded on XMLA path");

    match err {
        EngineError::QueryDeadlineExceeded {
            deadline_secs,
            ref hint,
            ..
        } => {
            assert_eq!(deadline_secs, 10, "deadline_secs should match config");
            assert!(!hint.is_empty(), "hint must be non-empty (FR6)");
        }
        other => panic!("expected QueryDeadlineExceeded, got: {other:?}"),
    }
}

/// AC2 (guardrail): A fast query that completes under the deadline returns
/// its rows byte-identical — the deadline wrapper adds no change to results.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fast_query_result_unchanged_under_deadline() {
    env::set_var("AC7_FAST_SECRET", "secret");
    let server = token_stub().await;
    let config = make_config(format!("{}/token", server.uri()), "AC7_FAST_SECRET", 60);
    let exec = LiveExecutor::with_row_source(config, Arc::new(FastRowSource));

    let result = exec
        .execute("SELECT fast", Backend::Sql, None, None)
        .expect("fast query should succeed under deadline");

    assert_eq!(result.rows.len(), 1, "should return 1 row");
    assert_eq!(result.rows[0]["col"], "fast-pgwire", "row content unchanged");
    assert!(!result.row_cap_tripped, "row_cap not tripped for 1 row");
}

/// AC6 (FR5): `resolve_deadline` clamps a per-request override above the max
/// to the configured maximum, never allowing a caller to disable the bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_deadline_clamps_to_max() {
    env::set_var("AC7_CLAMP_SECRET", "secret");
    let server = token_stub().await;
    let mut config = make_config(format!("{}/token", server.uri()), "AC7_CLAMP_SECRET", 60);
    config.query_deadline_max_secs = 90;
    let exec = LiveExecutor::with_row_source(config, Arc::new(FastRowSource));

    // A value below max passes through unchanged.
    assert_eq!(exec.resolve_deadline(Some(30)), 30, "below max: unchanged");

    // A value exactly at max passes through unchanged.
    assert_eq!(exec.resolve_deadline(Some(90)), 90, "at max: unchanged");

    // A value above max is clamped to the max (AC6, FR5).
    assert_eq!(
        exec.resolve_deadline(Some(300)),
        90,
        "above max: clamped to 90"
    );

    // None → server default.
    assert_eq!(
        exec.resolve_deadline(None),
        DEFAULT_QUERY_DEADLINE_SECS,
        "None: server default"
    );

    // Zero → server default (treated as no override).
    assert_eq!(
        exec.resolve_deadline(Some(0)),
        DEFAULT_QUERY_DEADLINE_SECS,
        "0: server default"
    );
}

/// FR4 (NFR2): Verify that the default deadline constant is 60s and the max
/// is 120s, matching the PRD spec. An unparseable/zero value must not result
/// in a "no deadline" state.
#[test]
fn deadline_defaults_match_spec() {
    assert_eq!(
        DEFAULT_QUERY_DEADLINE_SECS, 60,
        "default deadline must be 60s (PRD FR4)"
    );
    assert_eq!(
        DEFAULT_QUERY_DEADLINE_MAX_SECS, 120,
        "max deadline must be 120s (PRD FR4)"
    );
    // Critically, neither default is u64::MAX (which would disable the bound).
    assert_ne!(
        DEFAULT_QUERY_DEADLINE_SECS,
        u64::MAX,
        "default deadline must never be u64::MAX"
    );
    assert_ne!(
        DEFAULT_QUERY_DEADLINE_MAX_SECS,
        u64::MAX,
        "max deadline must never be u64::MAX"
    );
}

/// FR5 + resolve_deadline: per-request override via `execute_with_deadline`
/// passes the overridden deadline to the row source.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_with_deadline_passes_override_to_row_source() {
    env::set_var("AC7_OVERRIDE_SECRET", "secret");
    let server = token_stub().await;
    let config = make_config(
        format!("{}/token", server.uri()),
        "AC7_OVERRIDE_SECRET",
        60,
    );
    let capturing = Arc::new(DeadlineCapturingSource::default());
    let exec = LiveExecutor::with_row_source(config, capturing.clone());

    // Execute with an explicit per-request deadline of 30s (under max of 120).
    exec.execute_with_deadline("SELECT x", Backend::Sql, None, None, Some(30))
        .expect("should succeed");

    let captured = capturing
        .captured_pgwire_deadline
        .lock()
        .unwrap()
        .expect("deadline should have been captured");
    assert_eq!(captured, 30, "per-request override of 30s should reach RowSource");
}
