//! `mqo-parity` — CLI entry point for the MQO cross-backend parity oracle.
//!
//! # Usage
//!
//! ```text
//! mqo-parity --mqo <path> --catalog <path> [--backends dax,mdx,sql] [--require-live <backend>]
//! ```
//!
//! For each requested backend:
//! - `dax`, `mdx` → `Skipped (not yet wired)`
//! - `sql` → `Skipped (no live endpoint configured)`
//!
//! Emits the [`ParityReport`] as JSON on stdout.
//! Exits 0 when there are no non-skipped mismatches.
//! Exits 1 when `--require-live <backend>` names a backend that was skipped.
//! Exits 2 on a genuine parity mismatch among non-skipped backends.
//!
//! # Architecture note
//!
//! Backends will be wired to `mqo-auth-bridge` once the `Engine` trait is
//! stable. The `--require-live` flag is the production knob that makes this
//! oracle fail-fast when a required backend is unavailable.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process;

use clap::Parser;
use mqo_cross_backend_parity::{
    comparator::DaxComparator, run_parity, BackendStatus, OverallVerdict, ParityReport,
};

/// MQO cross-backend parity oracle.
///
/// Executes one MQO on each requested backend and asserts the results agree.
#[derive(Parser, Debug)]
#[command(name = "mqo-parity", version, about)]
struct Cli {
    /// Path to the MQO JSON file.
    #[arg(long)]
    mqo: PathBuf,

    /// Path to the catalog snapshot JSON (used for compiler grounding).
    #[arg(long)]
    catalog: PathBuf,

    /// Comma-separated list of backends to test. [default: sql]
    #[arg(long, default_value = "sql", value_delimiter = ',')]
    backends: Vec<String>,

    /// Treat a skipped instance of this backend as a hard failure (exit 1).
    ///
    /// Example: `--require-live sql` on a host where the SQL port is down.
    #[arg(long)]
    require_live: Vec<String>,
}

fn main() {
    let cli = Cli::parse();

    let mqo_path = cli.mqo.display().to_string();

    // Validate that the MQO file exists (catalog is noted but not yet used).
    if !cli.mqo.exists() {
        eprintln!("error: MQO file not found: {mqo_path}");
        process::exit(3);
    }

    // Read the MQO JSON (not yet compiled/executed; backends not wired).
    let _mqo_raw = match std::fs::read_to_string(&cli.mqo) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to read MQO file {mqo_path}: {e}");
            process::exit(3);
        }
    };

    // Build per-backend statuses.
    // TODO: when mqo-auth-bridge::Engine is wired, replace these stubs with
    //       actual compilation + execution per backend.
    let mut results: HashMap<String, BackendStatus> = HashMap::new();
    for backend in &cli.backends {
        let status = match backend.as_str() {
            "dax" => BackendStatus::Skipped {
                reason: "DAX backend not yet wired".to_string(),
            },
            "mdx" => BackendStatus::Skipped {
                reason: "MDX backend not yet wired".to_string(),
            },
            "sql" => BackendStatus::Skipped {
                reason: "SQL backend: no live endpoint configured".to_string(),
            },
            other => BackendStatus::Skipped {
                reason: format!("unknown backend: {other}"),
            },
        };
        results.insert(backend.clone(), status);
    }

    // Run parity comparison.
    let comparator = DaxComparator::default();
    let pairs = run_parity(&cli.backends, &results, &comparator);
    let report = ParityReport::build(
        mqo_path,
        cli.backends.clone(),
        results,
        pairs,
    );

    // Emit the report as JSON.
    let json = serde_json::to_string_pretty(&report).unwrap_or_else(|e| {
        eprintln!("error: failed to serialize report: {e}");
        process::exit(3);
    });
    println!("{json}");

    // Check --require-live constraints.
    for required in &cli.require_live {
        if let Some(status) = report.results.get(required) {
            if !status.is_executed() {
                eprintln!(
                    "error: required backend {required:?} was not live (skipped or errored)"
                );
                process::exit(1);
            }
        } else {
            eprintln!(
                "error: required backend {required:?} was not in --backends list"
            );
            process::exit(1);
        }
    }

    // Exit 2 on any genuine mismatch.
    if report.overall == OverallVerdict::Mismatch {
        process::exit(2);
    }

    process::exit(0);
}
