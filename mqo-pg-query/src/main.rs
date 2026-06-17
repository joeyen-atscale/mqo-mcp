//! `mqo-pg-query` — verbatim SQL executor over `AtScale` OIDC + `PGWire`.
//!
//! Executes a raw SQL statement against the `AtScale` `PGWire` endpoint using
//! the OIDC + TLS auth path from `mqo-auth-bridge`, and prints typed JSON rows.
//!
//! ## Usage
//!
//! ```text
//! mqo-pg-query --sql "SELECT ..." [connection flags]
//! mqo-pg-query [connection flags]   # reads SQL from stdin
//! ```
//!
//! ## Output
//!
//! - Success: `{"columns": [...], "rows": [[...], ...]}`
//! - Oversize: `{"oversize": {"observed_at_least": N, "cap": C}}`
//! - Error: `{"error": {"message": "..."}}` (exit non-zero)
//!
//! ## Secret handling
//!
//! Secrets are ALWAYS read from named env vars; never passed as flag values.
//! `--oidc-client-secret-env` names the env var holding the OIDC client secret.
//! `--pg-pass-env` names the env var holding the direct `PGWire` password.
//!
//! ## Exit codes
//!
//! - 0: success (table result or oversize signal)
//! - 1: query / auth / connection error
//! - 2: usage error (bad flags, empty SQL)

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use std::io::{self, Read};

use clap::Parser;

use mqo_pg_query::{config::QueryConfig, run_query, DEFAULT_MAX_ROWS};

/// Default OIDC token URL for the mcp-aws cluster.
const DEFAULT_TOKEN_URL: &str =
    "https://mcp-aws.atscaleinternal.com/auth/realms/atscale/protocol/openid-connect/token";

/// Default OIDC client ID for the mcp-aws cluster.
const DEFAULT_CLIENT_ID: &str = "atscale-mcp";

/// Default OIDC realm for the mcp-aws cluster.
const DEFAULT_REALM: &str = "atscale";

/// Default `PGWire` port for `AtScale`.
const DEFAULT_PG_PORT: u16 = 15432;

/// Default query timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Default env var name for the OIDC client secret.
const DEFAULT_SECRET_ENV: &str = "ATSCALE_OIDC_SECRET";

/// Verbatim SQL executor over `AtScale` OIDC + `PGWire`.
///
/// Authenticates via OIDC client-credentials (or ROPC with `--oidc-username`),
/// executes the SQL verbatim against the `AtScale` `PGWire` endpoint with TLS,
/// and prints typed JSON rows to stdout.
///
/// Secrets are read from the env vars named by the `*-env` flags — never as
/// direct flag values. Only the env-var name and `host:port` appear in output.
#[derive(Parser, Debug)]
#[command(
    name = "mqo-pg-query",
    about = "Verbatim SQL executor over AtScale OIDC+PGWire (gold-oracle CLI)",
    version,
    long_about = None,
)]
struct Args {
    // ── Query input ───────────────────────────────────────────────────────────
    /// SQL to execute. Reads from stdin when omitted.
    #[arg(long, value_name = "SQL")]
    sql: Option<String>,

    // ── Endpoint ──────────────────────────────────────────────────────────────
    /// `PGWire` host.
    #[arg(
        long,
        value_name = "HOST",
        default_value = "mcp-aws.atscaleinternal.com",
        env = "ATSCALE_PG_HOST"
    )]
    pg_host: String,

    /// `PGWire` port (default: 15432).
    #[arg(long, value_name = "PORT", default_value_t = DEFAULT_PG_PORT, env = "ATSCALE_PG_PORT")]
    pg_port: u16,

    // ── OIDC ──────────────────────────────────────────────────────────────────
    /// OIDC token endpoint URL.
    #[arg(
        long,
        value_name = "URL",
        default_value = DEFAULT_TOKEN_URL,
        env = "ATSCALE_OIDC_TOKEN_URL"
    )]
    oidc_token_url: String,

    /// OIDC client ID.
    #[arg(
        long,
        value_name = "ID",
        default_value = DEFAULT_CLIENT_ID,
        env = "ATSCALE_OIDC_CLIENT_ID"
    )]
    oidc_client_id: String,

    /// Name of the env var holding the OIDC client secret.
    /// The secret itself must NOT be passed as a flag value.
    #[arg(
        long,
        value_name = "VARNAME",
        default_value = DEFAULT_SECRET_ENV,
        env = "ATSCALE_OIDC_CLIENT_SECRET_ENV"
    )]
    oidc_client_secret_env: String,

    /// OIDC realm name.
    #[arg(
        long,
        value_name = "REALM",
        default_value = DEFAULT_REALM,
        env = "ATSCALE_OIDC_REALM"
    )]
    oidc_realm: String,

    /// OIDC username for ROPC grant (enables `grant_type=password`).
    #[arg(long, value_name = "USER", env = "ATSCALE_OIDC_USERNAME")]
    oidc_username: Option<String>,

    /// Name of the env var holding the ROPC user password.
    /// Only used when `--oidc-username` is set.
    #[arg(long, value_name = "VARNAME", env = "ATSCALE_OIDC_PASSWORD_ENV")]
    oidc_password_env: Option<String>,

    // ── Direct PGWire credentials (bypass OIDC on the PG connection) ──────────
    /// Override `PGWire` username (default: `"token"` for bearer-token auth).
    #[arg(long, value_name = "USER", env = "ATSCALE_PG_USER")]
    pg_user: Option<String>,

    /// Name of the env var holding the direct `PGWire` password.
    /// When set, skips OIDC token fetch for the `PGWire` connection.
    #[arg(long, value_name = "VARNAME", env = "ATSCALE_PG_PASS_ENV")]
    pg_pass_env: Option<String>,

    // ── Query controls ────────────────────────────────────────────────────────
    /// Maximum rows to return (default 50 000, ceiling 200 000).
    /// Results that exceed this emit `{"oversize": {...}}` instead of streaming.
    #[arg(
        long,
        value_name = "N",
        default_value_t = DEFAULT_MAX_ROWS,
        env = "ATSCALE_MAX_RESULT_ROWS"
    )]
    max_result_rows: usize,

    /// Per-query timeout in seconds (default 120).
    #[arg(
        long,
        value_name = "SECS",
        default_value_t = DEFAULT_TIMEOUT_SECS,
        env = "ATSCALE_QUERY_TIMEOUT_SECS"
    )]
    timeout_secs: u64,
}

fn main() {
    let args = Args::parse();

    // Resolve PGWire password from the named env var (never from a flag value).
    let pg_pass_resolved: Option<String> = if let Some(var_name) = &args.pg_pass_env {
        if let Ok(val) = std::env::var(var_name) {
            Some(val)
        } else {
            let msg = format!("env var '{var_name}' (named by --pg-pass-env) is not set");
            eprintln!("{}", serde_json::json!({"error": {"message": msg}}));
            std::process::exit(1);
        }
    } else {
        None
    };

    // Resolve SQL from --sql or stdin.
    let sql: String = if let Some(s) = args.sql {
        s
    } else {
        let mut buf = String::new();
        if let Err(e) = io::stdin().read_to_string(&mut buf) {
            let msg = format!("failed to read SQL from stdin: {e}");
            eprintln!("{}", serde_json::json!({"error": {"message": msg}}));
            std::process::exit(2);
        }
        buf
    };

    if sql.trim().is_empty() {
        let msg = "SQL is empty: provide --sql or pipe SQL on stdin";
        eprintln!("{}", serde_json::json!({"error": {"message": msg}}));
        std::process::exit(2);
    }

    let cfg = QueryConfig {
        pg_host: args.pg_host,
        pg_port: args.pg_port,
        oidc_token_url: args.oidc_token_url,
        oidc_client_id: args.oidc_client_id,
        oidc_client_secret_env: args.oidc_client_secret_env,
        oidc_realm: args.oidc_realm,
        oidc_username: args.oidc_username,
        oidc_password_env: args.oidc_password_env,
        pg_user: args.pg_user,
        pg_pass_resolved,
        max_result_rows: args.max_result_rows,
        timeout_secs: args.timeout_secs,
    };

    match run_query(&cfg, &sql) {
        Ok(output) => {
            println!("{}", output.to_json());
            std::process::exit(0);
        }
        Err(e) => {
            // Emit structured JSON error on stdout + human message on stderr (FR4).
            let json = serde_json::json!({"error": {"message": e.to_string()}});
            eprintln!("{json}");
            println!("{json}");
            std::process::exit(1);
        }
    }
}
