//! CLI configuration for `mqo-pg-query`.
//!
//! Auth params mirror `mqo-mcp-server`'s conventions. The OIDC client secret
//! is read from the env var named by `--oidc-client-secret-env`, never from a
//! flag value. Direct `PGWire` credentials (`--pg-user`, `--pg-pass-env`) take
//! priority over OIDC bearer-token auth on the `PGWire` connection.

/// Resolved query configuration (after flag parsing and env resolution).
///
/// All secret *values* are absent or resolved at runtime from env vars.
/// Only the env-var *names* and endpoint host:port appear here.
#[derive(Debug, Clone)]
pub struct QueryConfig {
    // ── Endpoint ──────────────────────────────────────────────────────────────
    /// `PGWire` host (e.g. `mcp-aws.atscaleinternal.com`).
    pub pg_host: String,
    /// `PGWire` port (default 15432).
    pub pg_port: u16,

    // ── OIDC (client-credentials or ROPC) ─────────────────────────────────────
    /// Full URL to the OIDC token endpoint.
    pub oidc_token_url: String,
    /// OIDC client ID.
    pub oidc_client_id: String,
    /// Name of env var holding the OIDC client secret. The secret is never stored here.
    pub oidc_client_secret_env: String,
    /// Keycloak realm name (informational; typically embedded in the token URL).
    pub oidc_realm: String,
    /// Optional ROPC username (enables `grant_type=password` instead of `client_credentials`).
    pub oidc_username: Option<String>,
    /// Name of env var holding the ROPC user password. Only used when `oidc_username` is set.
    pub oidc_password_env: Option<String>,

    // ── Direct PGWire credentials (override OIDC on the PG connection) ────────
    /// Override `PGWire` username. `None` → `"token"` (OIDC bearer auth).
    pub pg_user: Option<String>,
    /// Resolved `PGWire` password. `None` → use OIDC bearer token.
    /// The value is read from the env var named by `--pg-pass-env` at startup.
    pub pg_pass_resolved: Option<String>,

    // ── Query controls ────────────────────────────────────────────────────────
    /// Row cap (default [`crate::DEFAULT_MAX_ROWS`], ceiling [`crate::MAX_ROWS_CEILING`]).
    pub max_result_rows: usize,
    /// Query timeout in seconds (default 120).
    pub timeout_secs: u64,
}
