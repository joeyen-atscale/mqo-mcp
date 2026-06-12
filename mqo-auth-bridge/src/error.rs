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
}
