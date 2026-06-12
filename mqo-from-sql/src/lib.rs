//! Library interface for `mqo-from-sql` — usable from integration tests and
//! other crates.

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

pub mod error;
pub mod mqo_builder;
pub mod parser;
pub mod resolver;

use clap::CommandFactory;
use mqo_catalog_binder::catalog::CatalogSnapshot;
use mqo_spec::BoundMqo;

use error::MqoFromSqlError;

/// Parse an AtScale SQL projection string and resolve it against a catalog snapshot,
/// returning a fully-constructed `BoundMqo`.
///
/// This is the primary entry point for the compile pipeline.
///
/// # Errors
///
/// Returns [`MqoFromSqlError`] on parse, resolve, or validation failure.
pub fn compile_sql_with_snapshot(
    sql: &str,
    snapshot: &CatalogSnapshot,
) -> Result<BoundMqo, MqoFromSqlError> {
    let parsed = parser::parse_sql(sql)?;
    mqo_builder::build_bound_mqo(&parsed, snapshot)
}

// ── CLI introspection ─────────────────────────────────────────────────────────

/// The CLI `Args` definition — mirrors main.rs but defined in the lib so that
/// tests can inspect the argument surface without running the binary.
///
/// This is the authoritative source of truth for the arg surface.
#[derive(clap::Parser, Debug)]
#[command(name = "mqo-from-sql")]
struct CliArgs {
    /// Inline SQL string.
    #[arg(value_name = "SQL")]
    sql: Option<String>,

    /// CatalogSnapshot JSON file.
    #[arg(long, value_name = "FILE")]
    catalog: Option<String>,

    /// JSONL input file.
    #[arg(long, value_name = "FILE")]
    batch: Option<String>,

    /// Output file.
    #[arg(long, value_name = "FILE")]
    output: Option<String>,

    /// Output format.
    #[arg(long, default_value = "json")]
    format: String,

    /// Environment variable name for the catalog password (NOT the password itself).
    #[arg(long, value_name = "VARNAME")]
    pg_pass_env: Option<String>,
    // NOTE: --pg-pass is intentionally absent. Passwords must never appear on the
    // command line. Use --pg-pass-env to name an environment variable instead.
}

/// Return the full help text for the CLI argument surface.
///
/// Used by `credential_safety` tests to verify `--pg-pass` is not present.
#[must_use]
pub fn cli_help_text() -> String {
    let mut cmd = CliArgs::command();
    let mut buf = Vec::new();
    cmd.write_help(&mut buf).expect("clap writes help");
    String::from_utf8(buf).expect("help is UTF-8")
}
