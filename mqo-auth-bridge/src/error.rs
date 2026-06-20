//! Structured error type for all engine operations.

use thiserror::Error;

/// All errors that can arise during engine execution or token acquisition.
///
/// # Secret hygiene
///
/// No variant includes the secret value. The [`MissingSecret`] variant carries
/// only the *name* of the environment variable, never its contents.
///
/// [`MissingSecret`]: EngineError::MissingSecret
#[derive(Debug, Error)]
pub enum EngineError {
    /// The environment variable named by `OidcConfig.client_secret_env_var`
    /// was not set. Carries the var name, never the value.
    #[error("missing environment variable required for OIDC client secret: {var_name}")]
    MissingSecret {
        /// Name of the environment variable that was absent.
        var_name: String,
    },

    /// The OIDC token endpoint returned a non-success status or an
    /// unparseable response.
    #[error("OIDC authentication failed: {reason}")]
    AuthFailure {
        /// Human-readable description of the failure (no secrets).
        reason: String,
    },

    /// Could not establish a TCP/`PGWire`/XMLA connection to the `AtScale`
    /// endpoint.
    #[error("connection to AtScale endpoint failed: {reason}")]
    ConnectionFailure {
        /// Human-readable description (host:port, no secrets).
        reason: String,
    },

    /// The query was executed but the server returned an error response.
    #[error("query execution error: {reason}")]
    QueryError {
        /// Human-readable reason.
        reason: String,
    },

    /// Returned rows exceeded [`crate::HARD_ROW_CAP`]; the result was clamped.
    ///
    /// This is surfaced as an error variant so callers can react to the cap
    /// trip explicitly, mirroring the fixture engine's `rowLimitAdvisory`
    /// pattern.
    #[error("result set exceeded the hard row cap of {cap} rows; rows were truncated")]
    RowCapTripped {
        /// The hard cap that was applied.
        cap: usize,
    },

    /// HTTP transport error (from reqwest).
    #[error("HTTP transport error: {0}")]
    Http(#[from] reqwest::Error),

    /// `PGWire` / tokio-postgres error.
    #[error("PGWire error: {0}")]
    Postgres(#[from] tokio_postgres::Error),

    /// The query exceeded its per-request execution deadline.
    ///
    /// Returned when a `tokio::time::timeout` fires before the backend returns
    /// a result. The caller should surface `elapsed_secs`, `deadline_secs`,
    /// and `hint` to the agent so it can retry a cheaper shape or decline
    /// honestly — never retry the same shape on the same backend.
    ///
    /// On the PGWire path the warehouse `statement_timeout` GUC is set equal
    /// to the deadline before the query is issued, so the query is *cancelled*
    /// (not merely abandoned) when this fires. On the XMLA path cancellation
    /// is best-effort (the HTTP client is dropped).
    #[error(
        "query exceeded the {deadline_secs}s deadline (elapsed {elapsed_secs}s): {hint}"
    )]
    QueryDeadlineExceeded {
        /// Wall-clock seconds that elapsed before the deadline fired.
        elapsed_secs: u64,
        /// The configured deadline (server default or per-request override).
        deadline_secs: u64,
        /// One-line actionable hint for the agent or operator.
        hint: String,
    },
}
