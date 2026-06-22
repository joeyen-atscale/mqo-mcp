//! AC5: Backend dispatch tests.
//!
//! `LiveExecutor::execute` routes Sql to the PGWire path and Dax/Mdx to the
//! XMLA path. Verified via a `FakeRowSource` — no live cluster needed.
//!
//! Live-cluster assertions are skip-gated when ATSCALE_PGWIRE_HOST is absent.

use std::{
    env,
    sync::{Arc, Mutex},
};

use mqo_auth_bridge::{
    executor::{EndpointConfig, LiveExecutor},
    Backend, Engine, EngineError, OidcConfig,
};
use serde_json::json;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

// ─── Fake RowSource ───────────────────────────────────────────────────────────

/// Records which path was called (pgwire vs xmla) and returns synthetic rows.
#[derive(Debug, Default)]
struct FakeRowSource {
    calls: Mutex<Vec<&'static str>>,
    pgwire_rows: Vec<serde_json::Value>,
    xmla_rows: Vec<serde_json::Value>,
}

impl mqo_auth_bridge::executor::RowSource for FakeRowSource {
    fn pgwire_query(
        &self,
        _host: &str,
        _port: u16,
        _pg_user: &str,
        _pg_pass: &str,
        _query: &str,
        _limit: usize,
        _deadline_secs: u64,
    ) -> Result<Vec<serde_json::Value>, mqo_auth_bridge::EngineError> {
        self.calls.lock().unwrap().push("pgwire");
        Ok(self.pgwire_rows.clone())
    }

    fn xmla_query(
        &self,
        _xmla_url: &str,
        _bearer_token: &str,
        _query: &str,
        _catalog: &str,
        _cube: &str,
        _limit: usize,
        _deadline_secs: u64,
    ) -> Result<Vec<serde_json::Value>, mqo_auth_bridge::EngineError> {
        self.calls.lock().unwrap().push("xmla");
        Ok(self.xmla_rows.clone())
    }

    fn xmla_discover(
        &self,
        _xmla_url: &str,
        _bearer_token: &str,
    ) -> Result<(), mqo_auth_bridge::EngineError> {
        Ok(())
    }
}

async fn make_executor_with_token_stub(token_uri: String) -> (LiveExecutor, Arc<FakeRowSource>) {
    env::set_var("AC5_OIDC_SECRET", "secret");
    let config = EndpointConfig {
        pgwire_host: "localhost".to_string(),
        pgwire_port: 15432,
        xmla_url: "https://mcp-aws.atscaleinternal.com/v1/xmla".to_string(),
        oidc: OidcConfig {
            token_url: format!("{token_uri}/token"),
            client_id: "test-client".to_string(),
            client_secret_env_var: "AC5_OIDC_SECRET".to_string(),
            realm: "test".to_string(),
            username: None,
            password_env_var: None,
        },
        pg_user: None,
        pg_pass: None,
        max_result_rows: mqo_auth_bridge::DEFAULT_MAX_RESULT_ROWS,
        query_deadline_secs: mqo_auth_bridge::DEFAULT_QUERY_DEADLINE_SECS,
        query_deadline_max_secs: mqo_auth_bridge::DEFAULT_QUERY_DEADLINE_MAX_SECS,
        retry: Default::default(),
    };
    let fake = Arc::new(FakeRowSource {
        calls: Mutex::new(vec![]),
        pgwire_rows: vec![json!({"col": "pgwire-value"})],
        xmla_rows: vec![json!({"col": "xmla-value"})],
    });
    let exec = LiveExecutor::with_row_source(config, fake.clone());
    (exec, fake)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dax_routes_to_xmla() {
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

    let (exec, fake) = make_executor_with_token_stub(server.uri()).await;
    let result = exec
        .execute(
            "EVALUATE ROW(\"x\", 1)",
            Backend::Dax,
            Some(10),
            Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"),
        )
        .unwrap();
    assert_eq!(result.rows[0]["col"], "xmla-value");
    assert_eq!(*fake.calls.lock().unwrap(), vec!["xmla"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sql_routes_to_pgwire() {
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

    let (exec, fake) = make_executor_with_token_stub(server.uri()).await;
    let result = exec
        .execute(
            "SELECT SQL ...",
            Backend::Sql,
            Some(10),
            Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"),
        )
        .unwrap();
    assert_eq!(result.rows[0]["col"], "pgwire-value");
    assert_eq!(*fake.calls.lock().unwrap(), vec!["pgwire"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mdx_routes_to_xmla() {
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

    let (exec, fake) = make_executor_with_token_stub(server.uri()).await;
    let result = exec
        .execute(
            "SELECT MDX ...",
            Backend::Mdx,
            Some(10),
            Some("atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model"),
        )
        .unwrap();
    assert_eq!(result.rows[0]["col"], "xmla-value");
    assert_eq!(*fake.calls.lock().unwrap(), vec!["xmla"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dax_without_model_returns_error() {
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

    let (exec, _fake) = make_executor_with_token_stub(server.uri()).await;
    let err = exec
        .execute("EVALUATE ROW(\"x\", 1)", Backend::Dax, Some(10), None)
        .expect_err("should error when model is None for DAX");

    match err {
        EngineError::QueryError { reason } => {
            assert!(
                reason.contains("XMLA dispatch"),
                "expected 'XMLA dispatch' in reason, got: {reason}"
            );
        }
        other => panic!("expected QueryError, got: {other:?}"),
    }
}

#[test]
fn live_cluster_skip_gate() {
    if env::var("ATSCALE_PGWIRE_HOST").is_err() {
        println!("NOTE: live-cluster tests skipped — set ATSCALE_PGWIRE_HOST to enable them.");
        return;
    }
    // If the env is set, perform real connectivity smoke test (not implemented
    // here — a follow-on integration test file handles that).
    println!("ATSCALE_PGWIRE_HOST set; live smoke test would run here.");
}
