//! # mqo-pg-query
//!
//! Verbatim `PgSql` executor over `AtScale` OIDC + `PGWire`.
//!
//! Reuses `mqo-auth-bridge`'s OIDC client-credentials token flow and `PGWire`
//! TLS connection to execute raw SQL verbatim and emit typed JSON rows.
//! Designed for the eval harness gold-oracle path (`qwf20-ground-truth-oracle`).
//!
//! ## Output schema
//!
//! On success:
//! ```json
//! {"columns": ["col1", "col2"], "rows": [[val, val], ...]}
//! ```
//!
//! On over-cap (FR3):
//! ```json
//! {"oversize": {"observed_at_least": N, "cap": C}}
//! ```
//!
//! On error (FR4):
//! ```json
//! {"error": {"message": "..."}}
//! ```
//!
//! ## Secret handling (NFR1)
//!
//! All credentials are read from named environment variables — never from CLI
//! flag values, never logged. Only the env-var name and host:port appear in
//! output or errors.

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use mqo_auth_bridge::backend::Backend;
use mqo_auth_bridge::{EndpointConfig, Engine, LiveExecutor, OidcConfig};
use serde::Serialize;
use serde_json::Value;

pub mod config;
pub mod output;

pub use config::QueryConfig;
pub use output::{QueryOutput, ReferenceTable};

/// Default row cap (matches PRD FR3 default).
pub const DEFAULT_MAX_ROWS: usize = 50_000;

/// Hard ceiling for the row cap (matches PRD FR3 ceiling).
pub const MAX_ROWS_CEILING: usize = 200_000;

/// Errors from the query executor.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    /// A required env var was absent.
    #[error("missing environment variable '{var_name}': {detail}")]
    MissingEnvVar { var_name: String, detail: String },
    /// OIDC authentication failed.
    #[error("authentication failure: OIDC token fetch failed ({token_url})")]
    AuthFailure { token_url: String },
    /// `PGWire` connection failure.
    #[error("connection failure to {endpoint}: {message}")]
    ConnectionFailure { endpoint: String, message: String },
    /// SQL execution error.
    #[error("query execution error: {message}")]
    QueryFailure { message: String },
    /// Bad arguments / usage.
    #[error("usage error: {message}")]
    UsageError { message: String },
}

impl From<mqo_auth_bridge::EngineError> for QueryError {
    fn from(e: mqo_auth_bridge::EngineError) -> Self {
        use mqo_auth_bridge::EngineError;
        match e {
            EngineError::MissingSecret { var_name } => QueryError::MissingEnvVar {
                var_name,
                detail: "required for OIDC client secret".to_string(),
            },
            // AuthFailure: surface category only — no raw body (may contain token endpoint detail).
            EngineError::AuthFailure { .. } => QueryError::AuthFailure {
                token_url: "(see --oidc-token-url)".to_string(),
            },
            EngineError::ConnectionFailure { reason } => QueryError::ConnectionFailure {
                endpoint: "(see --endpoint)".to_string(),
                message: reason,
            },
            EngineError::QueryError { reason } => QueryError::QueryFailure { message: reason },
            EngineError::RowCapTripped { cap } => QueryError::QueryFailure {
                message: format!("row cap of {cap} was tripped during execution"),
            },
            EngineError::Http(inner) => QueryError::ConnectionFailure {
                endpoint: "(HTTP transport)".to_string(),
                message: inner.to_string(),
            },
            EngineError::Postgres(inner) => QueryError::QueryFailure {
                message: inner.to_string(),
            },
            EngineError::QueryDeadlineExceeded {
                elapsed_secs,
                deadline_secs,
                hint,
            } => QueryError::QueryFailure {
                message: format!(
                    "query exceeded the {deadline_secs}s deadline (elapsed {elapsed_secs}s): {hint}"
                ),
            },
            EngineError::EngineErrorRetriedExhausted {
                attempts,
                total_backoff_ms,
                message,
            } => QueryError::QueryFailure {
                message: format!(
                    "engine error persisted after {attempts} attempt(s) (total_backoff_ms={total_backoff_ms}): {message}"
                ),
            },
        }
    }
}

/// Run a verbatim SQL query against the `AtScale` `PGWire` endpoint.
///
/// Returns [`QueryOutput`] which serialises to the documented JSON schema.
///
/// # Errors
///
/// Returns [`QueryError`] on auth failure, connection failure, or SQL error.
/// Credentials are never included in the error.
pub fn run_query(cfg: &QueryConfig, sql: &str) -> Result<QueryOutput, QueryError> {
    if sql.trim().is_empty() {
        return Err(QueryError::UsageError {
            message: "SQL query is empty".to_string(),
        });
    }

    let cap = cfg.max_result_rows.clamp(1, MAX_ROWS_CEILING);

    let oidc = OidcConfig {
        token_url: cfg.oidc_token_url.clone(),
        client_id: cfg.oidc_client_id.clone(),
        client_secret_env_var: cfg.oidc_client_secret_env.clone(),
        realm: cfg.oidc_realm.clone(),
        // ROPC support: when oidc_username is set, the token flow uses
        // `grant_type=password` (direct user creds, per `qwf20-ground-truth-oracle`).
        username: cfg.oidc_username.clone(),
        password_env_var: cfg.oidc_password_env.clone(),
    };

    let endpoint = EndpointConfig {
        pgwire_host: cfg.pg_host.clone(),
        pgwire_port: cfg.pg_port,
        // XMLA URL is unused for raw SQL; provide a placeholder.
        xmla_url: format!("https://{}:{}/v1/xmla", cfg.pg_host, cfg.pg_port),
        oidc,
        // Direct user/pass override if provided (takes priority over OIDC bearer).
        pg_user: cfg.pg_user.clone(),
        pg_pass: cfg.pg_pass_resolved.clone(),
        // Fetch cap+1 so the executor can detect oversize without streaming all rows.
        max_result_rows: cap.saturating_add(1),
        // Use the oracle's own timeout_secs (sourced from --timeout-secs / config)
        // as the execution deadline so long analytic queries don't hang the oracle.
        // Default to the bridge default if not configured.
        query_deadline_secs: cfg.timeout_secs,
        query_deadline_max_secs: mqo_auth_bridge::DEFAULT_QUERY_DEADLINE_MAX_SECS,
        retry: Default::default(),
    };

    let executor = LiveExecutor::new(endpoint);

    // Execute verbatim SQL via the `PGWire` path (`Backend::Sql`).
    // The executor fetches up to `max_result_rows` (= cap+1) rows.
    // If it exceeds the budget it sets `row_cap_tripped = true`.
    let engine_result = executor
        .execute(
            sql,
            Backend::Sql,
            Some(u64::try_from(cap.saturating_add(1)).unwrap_or(u64::MAX)),
            None,
        )
        .map_err(QueryError::from)?;

    // Detect oversize (FR3): `row_cap_tripped` means real result > cap.
    if engine_result.row_cap_tripped {
        let observed = engine_result.rows.len();
        return Ok(QueryOutput::oversize(observed, cap));
    }

    // Convert rows: the executor returns `Vec<Value>` where each `Value` is an `Object`
    // (column_name → cell_value). Split into column headers + typed row arrays.
    let rows_obj = engine_result.rows;

    if rows_obj.is_empty() {
        // FR5: empty result is a valid table, not an error.
        return Ok(QueryOutput::Table(ReferenceTable {
            columns: vec![],
            rows: vec![],
        }));
    }

    // Extract column order from the first row.
    let Value::Object(first) = &rows_obj[0] else {
        return Err(QueryError::QueryFailure {
            message: "unexpected row format from `PGWire` executor".to_string(),
        });
    };
    let columns: Vec<String> = first.keys().cloned().collect();

    // Convert each row-object into an ordered array of typed cells.
    let rows: Vec<Vec<TypedCell>> = rows_obj
        .iter()
        .map(|row_val| {
            let Value::Object(map) = row_val else { return vec![] };
            columns
                .iter()
                .map(|col| {
                    let v = map.get(col).cloned().unwrap_or(Value::Null);
                    TypedCell::from_value(v)
                })
                .collect()
        })
        .collect();

    Ok(QueryOutput::Table(ReferenceTable { columns, rows }))
}

/// A typed cell value in the result table.
///
/// Mirrors the `mcp-eval-scoring-spec` cell types: null / integer / float / text.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum TypedCell {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
}

impl TypedCell {
    /// Convert a `serde_json::Value` into a typed cell.
    ///
    /// The `PGWire` executor performs numeric parsing (f64) on all values during
    /// the simple-query protocol decode. Here we further try integer before float.
    #[must_use]
    pub fn from_value(v: Value) -> Self {
        match v {
            Value::Null => TypedCell::Null,
            Value::Bool(b) => TypedCell::Text(b.to_string()),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    TypedCell::Int(i)
                } else if let Some(f) = n.as_f64() {
                    TypedCell::Float(f)
                } else {
                    TypedCell::Text(n.to_string())
                }
            }
            Value::String(s) => TypedCell::Text(s),
            // Complex types are stringified — should not appear from `PGWire`.
            Value::Array(_) | Value::Object(_) => TypedCell::Text(v.to_string()),
        }
    }
}
