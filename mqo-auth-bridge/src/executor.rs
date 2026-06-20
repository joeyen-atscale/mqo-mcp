//! Live query executor — dispatches to `PGWire` (DAX/SQL) or XMLA (MDX).
//!
//! # Testability convention
//!
//! Real `PGWire` and XMLA I/O is hidden behind the [`RowSource`] trait.
//! `LiveExecutor::execute` delegates to a `RowSource` implementation; in
//! production this is `WireRowSource` (actual network calls). In tests we
//! inject a fake via [`LiveExecutor::with_row_source`] so AC5/AC6 are
//! exercised without a live cluster. Network-dependent paths are skip-gated
//! behind `ATSCALE_PGWIRE_HOST` / `ATSCALE_XMLA_URL` env checks.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use futures::StreamExt;

use serde_json::Value;

use crate::{
    backend::Backend,
    engine::{Engine, EngineResult},
    error::EngineError,
    oidc::{OidcConfig, TokenCache},
};

// ─── Deadline defaults ────────────────────────────────────────────────────────

/// Default per-query execution deadline in seconds (FR4).
///
/// Well inside the 300s agent harness budget; leaves ≥ 240s for the agent to
/// recover, retry a cheaper shape, or decline honestly (G4).
/// Sourced from `--query-deadline-secs` / `MQO_QUERY_DEADLINE_SECS`.
pub const DEFAULT_QUERY_DEADLINE_SECS: u64 = 60;

/// Default upper bound for per-request deadline overrides (FR5).
///
/// A caller-supplied `deadline_secs` is silently clamped to this value so
/// it can never disable the execution bound entirely.
/// Sourced from `--query-deadline-max-secs`.
pub const DEFAULT_QUERY_DEADLINE_MAX_SECS: u64 = 120;

/// Actionable hint returned in [`EngineError::QueryDeadlineExceeded`] (FR6).
pub const DEADLINE_EXCEEDED_HINT: &str =
    "query exceeded the deadline; this MQO is likely cross-dimensional or \
     fine-grain — try a coarser grain, fewer projected levels, or a \
     measure-bearing shape.";

// ─── Configuration ───────────────────────────────────────────────────────────

/// Connection parameters for a live `AtScale` endpoint.
#[derive(Debug, Clone)]
pub struct EndpointConfig {
    /// Hostname for the `PGWire` listener (DAX/SQL path).
    pub pgwire_host: String,
    /// Port for the `PGWire` listener (typically 15432).
    pub pgwire_port: u16,
    /// Full URL to the XMLA engine endpoint (MDX path),
    /// e.g. `https://mcp-aws.atscaleinternal.com/v1/xmla`.
    pub xmla_url: String,
    /// OIDC configuration used to fetch bearer tokens.
    pub oidc: OidcConfig,
    /// Override `PGWire` username. When `None`, defaults to `"token"` (bearer-token auth).
    pub pg_user: Option<String>,
    /// Override `PGWire` password. When `Some`, skips OIDC token fetch entirely.
    pub pg_pass: Option<String>,
    /// Per-handle **materialization budget**: the maximum number of rows the
    /// executor fetches, truncates to, and lets the server persist into a
    /// handle. Replaces the old hard-coded 1000-row clamp
    /// (PRD-mqo-handle-full-materialization, FR-1/FR-2). When the real result
    /// exceeds this, the executor returns [`EngineResult::capped`]
    /// (`row_cap_tripped = true`) so the server can surface a typed over-budget
    /// signal instead of a silent clamp.
    ///
    /// Sourced from `--max-result-rows` (default [`DEFAULT_MAX_RESULT_ROWS`]),
    /// clamped to `1..=`[`MAX_RESULT_ROWS_CEILING`]. Set to 1000 to reproduce
    /// the pre-fix behavior exactly (rollback, AC-4).
    pub max_result_rows: usize,

    /// Per-query execution deadline in seconds (FR1–FR3, G1).
    ///
    /// Every backend execution (PGWire and XMLA) is wrapped in a
    /// `tokio::time::timeout` equal to this value. On elapse the executor
    /// returns [`EngineError::QueryDeadlineExceeded`] instead of hanging
    /// indefinitely. A `statement_timeout` equal to this value is also set on
    /// the PGWire session so the warehouse cancels the query (G3).
    ///
    /// Sourced from `--query-deadline-secs` / `MQO_QUERY_DEADLINE_SECS`
    /// (default [`DEFAULT_QUERY_DEADLINE_SECS`]). An unparseable or zero value
    /// falls back to the default with a warning (NFR2).
    pub query_deadline_secs: u64,

    /// Maximum value a caller may supply as a per-request deadline override
    /// (FR5). Overrides above this threshold are silently clamped; a warning
    /// is logged. Sourced from `--query-deadline-max-secs`
    /// (default [`DEFAULT_QUERY_DEADLINE_MAX_SECS`]).
    pub query_deadline_max_secs: u64,
}

// ─── Internal RowSource abstraction ─────────────────────────────────────────

/// Trait that abstracts the actual wire I/O.
///
/// The production implementation (`WireRowSource`) performs real network calls.
/// Tests inject a custom fake via [`LiveExecutor::with_row_source`].
pub trait RowSource: Send + Sync {
    /// Execute `query` against the `PGWire` endpoint. Returns at most `limit` rows.
    ///
    /// `pg_user` / `pg_pass` are already-resolved credentials (either
    /// `"token"` / OIDC bearer, or a direct email / password).
    ///
    /// `deadline_secs` is the per-query execution deadline (FR1–FR2). The
    /// implementation wraps the query in a `tokio::time::timeout` and, on the
    /// PGWire path, sets `statement_timeout` on the session before issuing the
    /// query. A deadline of `u64::MAX` disables the bound (rollback path).
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] on connection or query failure, or
    /// [`EngineError::QueryDeadlineExceeded`] when the deadline fires.
    #[allow(clippy::too_many_arguments)]
    fn pgwire_query(
        &self,
        host: &str,
        port: u16,
        pg_user: &str,
        pg_pass: &str,
        query: &str,
        limit: usize,
        deadline_secs: u64,
    ) -> Result<Vec<Value>, EngineError>;

    /// POST `query` to the XMLA endpoint and parse the cellset response.
    ///
    /// `deadline_secs` bounds the HTTP client timeout (FR3). A deadline of
    /// `u64::MAX` disables the bound.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] on HTTP or parse failure, or
    /// [`EngineError::QueryDeadlineExceeded`] when the deadline fires.
    #[allow(clippy::too_many_arguments)]
    fn xmla_query(
        &self,
        xmla_url: &str,
        bearer_token: &str,
        query: &str,
        catalog: &str,
        cube: &str,
        limit: usize,
        deadline_secs: u64,
    ) -> Result<Vec<Value>, EngineError>;

    /// Send a `DBSCHEMA_CATALOGS` Discover request to verify XMLA endpoint
    /// liveness.  Does not require a catalog or cube name.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] on network failure or a non-200 HTTP response.
    fn xmla_discover(&self, xmla_url: &str, bearer_token: &str) -> Result<(), EngineError>;
}

// ─── Production wire implementation ─────────────────────────────────────────

/// Production [`RowSource`] that performs real `PGWire` / XMLA network calls.
///
/// Actual execution uses a Tokio runtime for the async operations.
/// In tests this is replaced by a fake.
pub(crate) struct WireRowSource;

impl RowSource for WireRowSource {
    fn pgwire_query(
        &self,
        host: &str,
        port: u16,
        pg_user: &str,
        pg_pass: &str,
        query: &str,
        limit: usize,
        deadline_secs: u64,
    ) -> Result<Vec<Value>, EngineError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| EngineError::ConnectionFailure {
                reason: format!("failed to build tokio runtime: {e}"),
            })?;

        rt.block_on(async { pgwire_execute(host, port, pg_user, pg_pass, query, limit, deadline_secs).await })
    }

    fn xmla_query(
        &self,
        xmla_url: &str,
        bearer_token: &str,
        query: &str,
        catalog: &str,
        cube: &str,
        limit: usize,
        deadline_secs: u64,
    ) -> Result<Vec<Value>, EngineError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| EngineError::ConnectionFailure {
                reason: format!("failed to build tokio runtime: {e}"),
            })?;

        rt.block_on(async { xmla_execute(xmla_url, bearer_token, query, catalog, cube, limit, deadline_secs).await })
    }

    fn xmla_discover(&self, xmla_url: &str, bearer_token: &str) -> Result<(), EngineError> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| EngineError::ConnectionFailure {
                reason: format!("failed to build tokio runtime: {e}"),
            })?;
        rt.block_on(async { xmla_discover_catalogs(xmla_url, bearer_token).await })
    }
}

/// Execute a query over `PGWire` (DAX or SQL).
///
/// `AtScale` requires:
/// - TLS (self-signed cert accepted)
/// - Simple Query protocol (`simple_query`), not the extended protocol
///
/// Auth is either bearer-token (`user=token password=<oidc>`) or direct
/// (`user=<email> password=<pass>`), depending on what the caller passes.
///
/// `deadline_secs` wraps the entire execution (connection + query) in a
/// `tokio::time::timeout` and sets `statement_timeout` on the session so the
/// warehouse cancels an in-flight query on breach (FR1–FR2, G3). A deadline
/// of `u64::MAX` disables both bounds (rollback path). If the backend rejects
/// `SET statement_timeout`, the client-side `tokio` timeout still applies
/// (FR2 fallback); a one-line operator warning is logged.
async fn pgwire_execute(
    host: &str,
    port: u16,
    pg_user: &str,
    pg_pass: &str,
    query: &str,
    limit: usize,
    deadline_secs: u64,
) -> Result<Vec<Value>, EngineError> {
    use native_tls::TlsConnector;
    use postgres_native_tls::MakeTlsConnector;
    use tokio::time::{timeout, Duration};

    let pg_dbname = std::env::var("ATSCALE_PG_DBNAME")
        .unwrap_or_else(|_| "atscale_catalogs".to_string());
    let pg_sslmode = std::env::var("ATSCALE_PG_SSLMODE")
        .unwrap_or_else(|_| "require".to_string());
    let conn_str = format!(
        "host={host} port={port} dbname={pg_dbname} user={pg_user} password={pg_pass} sslmode={pg_sslmode}"
    );

    let tls = MakeTlsConnector::new(
        TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|e| EngineError::ConnectionFailure {
                reason: format!("TLS builder error: {e}"),
            })?,
    );

    let start = Instant::now();

    // Build the deadline duration; u64::MAX is the "no deadline" sentinel.
    let deadline = if deadline_secs == u64::MAX {
        None
    } else {
        Some(Duration::from_secs(deadline_secs))
    };

    // Wrap connect + query in the client-side deadline (FR1).
    let execute_fut = async {
        let (client, connection) = tokio_postgres::connect(&conn_str, tls).await?;

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("pgwire connection error: {e}");
            }
        });

        // Set warehouse-side statement_timeout so the query is *cancelled* on
        // breach, not just abandoned (FR2, G3). If the backend rejects this GUC,
        // fall back gracefully (client-side timeout still applies — NFR2, AC4).
        if let Some(d) = deadline {
            let timeout_ms = d.as_millis();
            let set_stmt = format!("SET statement_timeout = {timeout_ms}");
            if let Err(e) = client.simple_query(&set_stmt).await {
                eprintln!(
                    "event=statement_timeout_set_failed backend=pgwire deadline={deadline_secs} err={e}; \
                     falling back to client-side tokio deadline only"
                );
                // Non-fatal: client-side deadline (FR1) still applies.
            }
        }

        // AtScale PGWire requires the simple (text-only) query protocol.
        let messages = client.simple_query(query).await?;

        let mut result: Vec<Value> = Vec::new();
        for msg in messages {
            if let tokio_postgres::SimpleQueryMessage::Row(row) = msg {
                if result.len() >= limit {
                    break;
                }
                let mut obj = serde_json::Map::new();
                for (i, col) in row.columns().iter().enumerate() {
                    let v = match row.get(i) {
                        Some(s) => {
                            // Try numeric parse; fall back to string.
                            if let Ok(n) = s.parse::<f64>() {
                                serde_json::json!(n)
                            } else {
                                Value::String(s.to_string())
                            }
                        }
                        None => Value::Null,
                    };
                    obj.insert(col.name().to_string(), v);
                }
                result.push(Value::Object(obj));
            }
        }

        Ok::<Vec<Value>, EngineError>(result)
    };

    match deadline {
        None => execute_fut.await,
        Some(d) => match timeout(d, execute_fut).await {
            Ok(result) => result,
            Err(_elapsed) => {
                let elapsed_secs = start.elapsed().as_secs();
                eprintln!(
                    "event=query_deadline_exceeded backend=pgwire elapsed={elapsed_secs} deadline={deadline_secs}"
                );
                Err(EngineError::QueryDeadlineExceeded {
                    elapsed_secs,
                    deadline_secs,
                    hint: DEADLINE_EXCEEDED_HINT.to_string(),
                })
            }
        },
    }
}

/// POST an MDX or DAX query to the XMLA endpoint and parse the response.
///
/// Builds a proper SOAP/XMLA `Execute` envelope with `Catalog`, `Cube`,
/// `<Format>Tabular</Format>`, and `<Content>Data</Content>`, POSTs it with
/// the bearer token, and delegates response parsing to
/// [`crate::xmla::parse_xmla_cellset`].  Both the Tabular rowset and
/// `<MDDataSet>` response shapes are handled there.  A parse failure or a SOAP
/// `<Fault>` always returns `Err` — synthetic rows are never fabricated.
///
/// `deadline_secs` sets the HTTP client timeout (FR3). On elapse the HTTP
/// client is dropped (best-effort XMLA cancellation — NG4) and
/// [`EngineError::QueryDeadlineExceeded`] is returned.
async fn xmla_execute(
    xmla_url: &str,
    bearer_token: &str,
    query: &str,
    catalog: &str,
    cube: &str,
    limit: usize,
    deadline_secs: u64,
) -> Result<Vec<Value>, EngineError> {
    use std::time::Duration;

    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body>
    <Execute xmlns="urn:schemas-microsoft-com:xml-analysis">
      <Command><Statement>{}</Statement></Command>
      <Properties>
        <PropertyList>
          <Catalog>{}</Catalog>
          <Cube>{}</Cube>
          <Format>Tabular</Format>
          <Content>Data</Content>
        </PropertyList>
      </Properties>
    </Execute>
  </soap:Body>
</soap:Envelope>"#,
        xmla_escape(query),
        xmla_escape(catalog),
        xmla_escape(cube),
    );

    let start = Instant::now();

    // Build the reqwest client with or without a connection/request timeout.
    // A timeout fires as a reqwest::Error; we map it to QueryDeadlineExceeded
    // below (FR3). u64::MAX is the "no deadline" sentinel (rollback path).
    let mut client_builder = reqwest::Client::builder();
    if deadline_secs != u64::MAX {
        client_builder = client_builder.timeout(Duration::from_secs(deadline_secs));
    }
    let client = client_builder.build().map_err(|e| EngineError::ConnectionFailure {
        reason: format!("XMLA reqwest client build failed: {e}"),
    })?;

    let send_result = client
        .post(xmla_url)
        .header("Authorization", format!("Bearer {bearer_token}"))
        .header("Content-Type", "application/xml")
        .body(body)
        .send()
        .await;

    // Map a reqwest timeout to QueryDeadlineExceeded (FR3).
    let resp = match send_result {
        Ok(r) => r,
        Err(e) if e.is_timeout() => {
            let elapsed_secs = start.elapsed().as_secs();
            eprintln!(
                "event=query_deadline_exceeded backend=xmla elapsed={elapsed_secs} deadline={deadline_secs}"
            );
            return Err(EngineError::QueryDeadlineExceeded {
                elapsed_secs,
                deadline_secs,
                hint: DEADLINE_EXCEEDED_HINT.to_string(),
            });
        }
        Err(e) => return Err(EngineError::Http(e)),
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(EngineError::QueryError {
            reason: format!("XMLA endpoint returned {status}: {text}"),
        });
    }

    let text = resp.text().await?;
    crate::xmla::parse_xmla_cellset(&text, limit)
}

/// Escape XML special characters in an MDX statement.
fn xmla_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Send a `DBSCHEMA_CATALOGS` Discover to verify XMLA endpoint liveness.
///
/// Model-agnostic — no catalog or cube required.  Returns `Ok(())` on HTTP 200.
async fn xmla_discover_catalogs(xmla_url: &str, bearer_token: &str) -> Result<(), EngineError> {
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
  <soap:Body>
    <Discover xmlns="urn:schemas-microsoft-com:xml-analysis">
      <RequestType>DBSCHEMA_CATALOGS</RequestType>
      <Restrictions><RestrictionList></RestrictionList></Restrictions>
      <Properties><PropertyList></PropertyList></Properties>
    </Discover>
  </soap:Body>
</soap:Envelope>"#;

    let client = reqwest::Client::new();
    let resp = client
        .post(xmla_url)
        .header("Authorization", format!("Bearer {bearer_token}"))
        .header("Content-Type", "application/xml")
        .body(body)
        .send()
        .await?;

    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Err(EngineError::QueryError {
            reason: format!("XMLA DISCOVER returned HTTP {status}: {text}"),
        })
    }
}

// ─── MDSCHEMA discovery (live catalog ingestion) ──────────────────────────────

/// Decode the basic XML entities that appear in XMLA rowset values.
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Parse an XMLA `Tabular`-format Discover response into one map per `<row>`
/// (flat `<TAG>value</TAG>` pairs; nested tags are skipped). Minimal,
/// dependency-free — the MDSCHEMA rowsets we consume are flat.
fn parse_xmla_rows(xml: &str) -> Vec<BTreeMap<String, String>> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(s) = rest.find("<row>") {
        let after = &rest[s + "<row>".len()..];
        let Some(e) = after.find("</row>") else { break };
        let row = &after[..e];
        let mut map = BTreeMap::new();
        let mut r = row;
        while let Some(ts) = r.find('<') {
            let after_lt = &r[ts + 1..];
            let Some(te) = after_lt.find('>') else { break };
            let tag = &after_lt[..te];
            if tag.starts_with('/') || tag.ends_with('/') {
                r = &after_lt[te + 1..]; // closing or self-closing tag
                continue;
            }
            let close = format!("</{tag}>");
            let content = &after_lt[te + 1..];
            if let Some(ce) = content.find(&close) {
                let val = &content[..ce];
                if !val.contains('<') {
                    map.insert(tag.to_string(), xml_unescape(val));
                }
                r = &content[ce + close.len()..];
            } else {
                r = content;
            }
        }
        out.push(map);
        rest = &after[e + "</row>".len()..];
    }
    out
}

/// Issue an MDSCHEMA Discover and return its parsed rows. `level` sets a
/// `LEVEL_UNIQUE_NAME` restriction (for `MDSCHEMA_MEMBERS`); pass `None` otherwise.
async fn xmla_discover_rows(
    xmla_url: &str,
    bearer_token: &str,
    request_type: &str,
    catalog: &str,
    cube: &str,
    level: Option<&str>,
) -> Result<Vec<BTreeMap<String, String>>, EngineError> {
    let mut restrictions = format!(
        "<CATALOG_NAME>{}</CATALOG_NAME><CUBE_NAME>{}</CUBE_NAME>",
        xmla_escape(catalog),
        xmla_escape(cube)
    );
    if let Some(l) = level {
        use std::fmt::Write as _;
        let _ = write!(
            restrictions,
            "<LEVEL_UNIQUE_NAME>{}</LEVEL_UNIQUE_NAME>",
            xmla_escape(l)
        );
    }
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body>
<Discover xmlns="urn:schemas-microsoft-com:xml-analysis">
<RequestType>{request_type}</RequestType>
<Restrictions><RestrictionList>{restrictions}</RestrictionList></Restrictions>
<Properties><PropertyList><Catalog>{catalog}</Catalog><Format>Tabular</Format></PropertyList></Properties>
</Discover></soap:Body></soap:Envelope>"#,
        catalog = xmla_escape(catalog),
    );

    let client = reqwest::Client::new();
    let resp = client
        .post(xmla_url)
        .header("Authorization", format!("Bearer {bearer_token}"))
        .header("Content-Type", "application/xml")
        .body(body)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(EngineError::QueryError {
            reason: format!("XMLA {request_type} returned HTTP {status}: {text}"),
        });
    }
    let text = resp.text().await?;
    Ok(parse_xmla_rows(&text))
}

// ─── Model path helpers ───────────────────────────────────────────────────────

/// Parse `<Catalog>` and `<Cube>` from a dot-separated MQO model path.
///
/// `atscale_catalogs.tpcds_Databricks.tpcds_benchmark_model`
///  → catalog = `"tpcds_Databricks"`, cube = `"tpcds_benchmark_model"`
///
/// Returns `Err` if the string has fewer than 3 dot-segments.
fn parse_model_catalog_cube(model: &str) -> Result<(&str, &str), EngineError> {
    let parts: Vec<&str> = model.splitn(4, '.').collect();
    if parts.len() < 3 {
        return Err(EngineError::QueryError {
            reason: format!(
                "cannot derive XMLA catalog/cube from model path '{model}': \
                 expected at least 3 dot-segments (e.g. 'atscale_catalogs.schema.model')"
            ),
        });
    }
    Ok((parts[1], parts[2]))
}

// ─── LiveExecutor ─────────────────────────────────────────────────────────────

/// Live query executor.
///
/// Authenticates via OIDC client-credentials and dispatches compiled queries
/// to a live `AtScale` endpoint:
///
/// - `Dax` / `Mdx` → XMLA path (`xmla_url`)
/// - `Sql` → `PGWire` path (`pgwire_host:pgwire_port`)
///
/// Row results are clamped to the configured materialization budget
/// (`EndpointConfig::max_result_rows`).
pub struct LiveExecutor {
    config: EndpointConfig,
    token_cache: TokenCache,
    row_source: Arc<dyn RowSource>,
}

impl std::fmt::Debug for LiveExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveExecutor")
            .field("config", &self.config)
            .field("token_cache", &self.token_cache)
            .finish_non_exhaustive()
    }
}

impl LiveExecutor {
    /// Create a `LiveExecutor` using the real `PGWire`/XMLA wire implementations.
    #[must_use]
    pub fn new(config: EndpointConfig) -> Self {
        let token_cache = TokenCache::new(config.oidc.clone());
        Self {
            config,
            token_cache,
            row_source: Arc::new(WireRowSource),
        }
    }

    /// Create a `LiveExecutor` with a custom [`RowSource`] — used in tests to
    /// inject a fake without a live cluster.
    #[must_use]
    pub fn with_row_source(config: EndpointConfig, row_source: Arc<dyn RowSource>) -> Self {
        let token_cache = TokenCache::new(config.oidc.clone());
        Self {
            config,
            token_cache,
            row_source,
        }
    }

    /// Fetch a fresh (or cached) bearer token.
    ///
    /// Works from both sync and async contexts:
    /// - Inside a Tokio runtime → `block_in_place` (multi-thread runtime),
    ///   spawning the async work on a separate task.
    /// - Outside any runtime → builds a temporary current-thread runtime.
    ///
    /// # Errors
    ///
    /// Propagates [`EngineError::MissingSecret`], [`EngineError::AuthFailure`],
    /// or [`EngineError::Http`].
    ///
    /// # Panics
    ///
    /// May panic if the spawned token-fetch task itself panics (propagated as
    /// [`EngineError::ConnectionFailure`]).
    pub fn fetch_token_sync(&self) -> Result<crate::oidc::Token, EngineError> {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            // We are inside a runtime. Use block_in_place so we don't
            // deadlock: spawn the async work on a dedicated task and block.
            let cache = self.token_cache.clone();
            let join = handle.spawn(async move { cache.fetch_token().await });
            tokio::task::block_in_place(|| {
                handle
                    .block_on(join)
                    .map_err(|e| EngineError::ConnectionFailure {
                        reason: format!("token fetch task panicked: {e}"),
                    })?
            })
        } else {
            // No runtime present; create a temporary one.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| EngineError::ConnectionFailure {
                    reason: format!("failed to build tokio runtime for token fetch: {e}"),
                })?;
            rt.block_on(self.token_cache.fetch_token())
        }
    }

    /// Issue an MDSCHEMA Discover against this executor's XMLA endpoint and
    /// return the parsed rows. Mints a bearer token via [`Self::fetch_token_sync`].
    /// Used by live catalog ingestion (PRD-mqo-live-catalog-ingestion):
    /// `MDSCHEMA_MEASURES` / `MDSCHEMA_LEVELS` for the column metadata, and
    /// `MDSCHEMA_MEMBERS` (with `level`) for a low-cardinality level's domain.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the token fetch fails, the HTTP request
    /// fails, or the XMLA response cannot be parsed.
    pub fn discover_mdschema(
        &self,
        request_type: &str,
        catalog: &str,
        cube: &str,
        level: Option<&str>,
    ) -> Result<Vec<BTreeMap<String, String>>, EngineError> {
        let token = self.fetch_token_sync()?;
        let url = self.config.xmla_url.clone();
        let rt = request_type.to_string();
        let cat = catalog.to_string();
        let cb = cube.to_string();
        let lv = level.map(str::to_string);
        let fut = async move {
            xmla_discover_rows(&url, &token.access_token, &rt, &cat, &cb, lv.as_deref()).await
        };
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let join = handle.spawn(fut);
            tokio::task::block_in_place(|| {
                handle.block_on(join).map_err(|e| EngineError::ConnectionFailure {
                    reason: format!("mdschema discover task panicked: {e}"),
                })?
            })
        } else {
            let rt2 = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| EngineError::ConnectionFailure {
                    reason: format!("failed to build tokio runtime for mdschema discover: {e}"),
                })?;
            rt2.block_on(fut)
        }
    }

    /// Fetch `MDSCHEMA_MEMBERS` for multiple levels concurrently.
    ///
    /// Fetches the bearer token **once**, then fans out up to `concurrency`
    /// simultaneous `xmla_discover_rows` calls using `buffer_unordered`.
    /// Returns a `Vec` of `(key, Result<rows>)` in completion order; the caller
    /// is responsible for ordering / `BTreeMap` insertion.
    ///
    /// Per-level errors are returned as `Err` entries (fail-open); the batch
    /// never aborts on a single failure. `concurrency == 1` serializes the
    /// fetches exactly like the old loop.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the token fetch fails or the Tokio runtime
    /// cannot be acquired.  Per-level errors are returned as `Err` in the
    /// result `Vec`, not as the outer `Err`.
    #[allow(clippy::type_complexity)]
    pub fn discover_members_batch(
        &self,
        levels: &[((String, String), String)],
        catalog: &str,
        cube: &str,
        concurrency: usize,
    ) -> Result<Vec<((String, String), Result<Vec<BTreeMap<String, String>>, EngineError>)>, EngineError> {
        let token = self.fetch_token_sync()?;
        let access_token = token.access_token;
        let xmla_url = self.config.xmla_url.clone();
        let cat = catalog.to_string();
        let cb = cube.to_string();
        let conc = concurrency.max(1);

        // Build one future per level; each yields (key, Result<rows>).
        let futures_iter = levels.iter().map(|(key, lun)| {
            let url = xmla_url.clone();
            let tok = access_token.clone();
            let cat2 = cat.clone();
            let cb2 = cb.clone();
            let lun2 = lun.clone();
            let key2 = key.clone();
            async move {
                let result =
                    xmla_discover_rows(&url, &tok, "MDSCHEMA_MEMBERS", &cat2, &cb2, Some(&lun2))
                        .await;
                (key2, result)
            }
        });

        let stream = futures::stream::iter(futures_iter).buffer_unordered(conc);

        // Drive the bounded stream synchronously — we are in a sync caller.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            Ok(tokio::task::block_in_place(|| {
                handle.block_on(stream.collect::<Vec<_>>())
            }))
        } else {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| EngineError::ConnectionFailure {
                    reason: format!("failed to build tokio runtime for batch discover: {e}"),
                })?;
            Ok(rt.block_on(stream.collect::<Vec<_>>()))
        }
    }
}

impl Engine for LiveExecutor {
    fn ping_xmla(&self) -> Result<(), EngineError> {
        let token = self.fetch_token_sync()?;
        self.row_source
            .xmla_discover(&self.config.xmla_url, &token.access_token)
    }

    fn execute(
        &self,
        compiled_query: &str,
        backend: Backend,
        limit: Option<u64>,
        model: Option<&str>,
    ) -> Result<EngineResult, EngineError> {
        // The materialization budget is the persisted-handle ceiling
        // (PRD-mqo-handle-full-materialization). Fetch up to `budget + 1` so an
        // over-budget result is *detectable* (the +1 distinguishes
        // "exactly at budget" from "exceeded it"), then truncate to the budget.
        let budget = self.config.max_result_rows.max(1);
        let fetch_limit = budget.saturating_add(1);
        // A caller-supplied `limit` is an *intentional* bound, not a truncation:
        // it never trips `row_cap_tripped`. It is still clamped by the budget.
        let user_limit = limit
            .map_or(budget, |l| usize::try_from(l).unwrap_or(budget))
            .min(budget);

        // Use the server-level deadline for this execute call.
        // Per-request overrides are handled by execute_with_deadline (FR5).
        let deadline_secs = self.config.query_deadline_secs;

        let raw_rows = match backend {
            Backend::Sql => {
                // PGWire: direct credentials take priority over OIDC token.
                let (pg_user, pg_pass_owned);
                let pg_pass: &str = if let Some(ref p) = self.config.pg_pass {
                    pg_user = self.config.pg_user.as_deref().unwrap_or("token");
                    p.as_str()
                } else {
                    let token = self.fetch_token_sync()?;
                    pg_pass_owned = token.access_token;
                    pg_user = self.config.pg_user.as_deref().unwrap_or("token");
                    pg_pass_owned.as_str()
                };
                self.row_source.pgwire_query(
                    &self.config.pgwire_host,
                    self.config.pgwire_port,
                    pg_user,
                    pg_pass,
                    compiled_query,
                    fetch_limit,
                    deadline_secs,
                )?
            }
            Backend::Dax | Backend::Mdx => {
                let (catalog, cube) = match model {
                    Some(m) => parse_model_catalog_cube(m)?,
                    None => return Err(EngineError::QueryError {
                        reason: "XMLA dispatch (DAX/MDX) requires a model path but none was provided".to_string(),
                    }),
                };
                // XMLA always uses an OIDC bearer token — never a raw PGWire password.
                // This is correct even when --pg-user/--pg-pass-env direct auth is active.
                let token = self.fetch_token_sync()?;
                self.row_source.xmla_query(
                    &self.config.xmla_url,
                    &token.access_token,
                    compiled_query,
                    catalog,
                    cube,
                    fetch_limit,
                    deadline_secs,
                )?
            }
        };

        // Over-budget: the real result exceeded the materialization budget.
        // Truncate to the budget and trip the flag so the server surfaces a
        // typed over-budget signal — never a silent clamp presented as complete
        // (FR-3). Detected via the extra fetched row (len > budget).
        if raw_rows.len() > budget {
            let rows: Vec<Value> = raw_rows.into_iter().take(budget).collect();
            return Ok(EngineResult::capped(rows));
        }

        // Within budget. A caller-supplied `limit` smaller than the result is an
        // intentional bound (e.g. top-N), not a truncation of the full set — it
        // does NOT trip `row_cap_tripped`.
        if raw_rows.len() > user_limit {
            let rows: Vec<Value> = raw_rows.into_iter().take(user_limit).collect();
            return Ok(EngineResult::new(rows));
        }

        Ok(EngineResult::new(raw_rows))
    }
}

impl LiveExecutor {
    /// Resolve a per-request deadline override against the server maximum (FR5).
    ///
    /// - `None` → use `self.config.query_deadline_secs` (server default).
    /// - `Some(n)` → clamp to `self.config.query_deadline_max_secs`; log a
    ///   warning if clamping occurred (AC6).
    /// - `Some(0)` → treated as "no override" (server default applies).
    #[must_use]
    pub fn resolve_deadline(&self, per_request: Option<u64>) -> u64 {
        match per_request {
            None | Some(0) => self.config.query_deadline_secs,
            Some(n) => {
                let max = self.config.query_deadline_max_secs;
                if n > max {
                    eprintln!(
                        "event=deadline_override_clamped requested={n} max={max}; \
                         clamped to {max} (FR5)"
                    );
                    max
                } else {
                    n
                }
            }
        }
    }

    /// Execute `compiled_query` with an explicit per-request deadline override.
    ///
    /// Identical to [`Engine::execute`] but accepts an optional
    /// `deadline_secs_override` that replaces the server-level default for this
    /// one call, clamped to `query_deadline_max_secs` (FR5). Pass `None` to use
    /// the server default.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] including
    /// [`EngineError::QueryDeadlineExceeded`] on a breach.
    pub fn execute_with_deadline(
        &self,
        compiled_query: &str,
        backend: Backend,
        limit: Option<u64>,
        model: Option<&str>,
        deadline_secs_override: Option<u64>,
    ) -> Result<EngineResult, EngineError> {
        let deadline_secs = self.resolve_deadline(deadline_secs_override);

        let budget = self.config.max_result_rows.max(1);
        let fetch_limit = budget.saturating_add(1);
        let user_limit = limit
            .map_or(budget, |l| usize::try_from(l).unwrap_or(budget))
            .min(budget);

        let raw_rows = match backend {
            Backend::Sql => {
                let (pg_user, pg_pass_owned);
                let pg_pass: &str = if let Some(ref p) = self.config.pg_pass {
                    pg_user = self.config.pg_user.as_deref().unwrap_or("token");
                    p.as_str()
                } else {
                    let token = self.fetch_token_sync()?;
                    pg_pass_owned = token.access_token;
                    pg_user = self.config.pg_user.as_deref().unwrap_or("token");
                    pg_pass_owned.as_str()
                };
                self.row_source.pgwire_query(
                    &self.config.pgwire_host,
                    self.config.pgwire_port,
                    pg_user,
                    pg_pass,
                    compiled_query,
                    fetch_limit,
                    deadline_secs,
                )?
            }
            Backend::Dax | Backend::Mdx => {
                let (catalog, cube) = match model {
                    Some(m) => parse_model_catalog_cube(m)?,
                    None => {
                        return Err(EngineError::QueryError {
                            reason: "XMLA dispatch (DAX/MDX) requires a model path but none was provided".to_string(),
                        })
                    }
                };
                let token = self.fetch_token_sync()?;
                self.row_source.xmla_query(
                    &self.config.xmla_url,
                    &token.access_token,
                    compiled_query,
                    catalog,
                    cube,
                    fetch_limit,
                    deadline_secs,
                )?
            }
        };

        if raw_rows.len() > budget {
            let rows: Vec<Value> = raw_rows.into_iter().take(budget).collect();
            return Ok(EngineResult::capped(rows));
        }
        if raw_rows.len() > user_limit {
            let rows: Vec<Value> = raw_rows.into_iter().take(user_limit).collect();
            return Ok(EngineResult::new(rows));
        }
        Ok(EngineResult::new(raw_rows))
    }
}
