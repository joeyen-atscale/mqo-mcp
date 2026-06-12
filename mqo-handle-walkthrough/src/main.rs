//! mqo-handle-walkthrough — 4-turn POC session with zero-requery assertion.

use std::fs;
use std::path::PathBuf;

use clap::Parser;
use mqo_duckdb_handle_store::MemStore;
use mqo_handle_walkthrough::walkthrough::run_default_script;
use serde_json::Value;

#[derive(Parser, Debug)]
#[command(
    name = "mqo-handle-walkthrough",
    about = "4-turn MQO walkthrough: query → YoY → slice → chart, asserting zero re-queries",
    version
)]
struct Cli {
    /// Live mqo-mcp-server command (optional; skips to offline mode when absent).
    #[arg(long)]
    server: Option<String>,

    /// Offline seed rows JSON file (used when --server is absent).
    #[arg(long, value_name = "rows.json")]
    seed_result: Option<PathBuf>,

    /// Handle store backend: mem (default) or duckdb (requires duckdb feature).
    #[arg(long, default_value = "mem")]
    store: String,

    /// Override walkthrough script JSON (optional).
    #[arg(long, value_name = "walkthrough.json")]
    script: Option<PathBuf>,

    /// Output directory for transcript + Vega-Lite spec.
    #[arg(long, value_name = "dir", default_value = ".")]
    out: PathBuf,
}

fn main() {
    let cli = Cli::parse();

    // ── Determine seed rows ─────────────────────────────────────────────────
    let (seed_rows, requery_count) = if let Some(ref _server_cmd) = cli.server {
        // Live path: gate behind env vars; credentials NEVER written to any file.
        let host = std::env::var("ATSCALE_PGWIRE_HOST").ok();
        let pass = std::env::var("ATSCALE_PG_PASS").ok();
        if host.is_none() || pass.is_none() {
            eprintln!(
                "SKIP: --server supplied but ATSCALE_PGWIRE_HOST / ATSCALE_PG_PASS not set; \
                 running offline with built-in seed."
            );
            (load_builtin_seed(), 1usize)
        } else {
            eprintln!("Live server mode not yet implemented in this POC; running offline.");
            (load_builtin_seed(), 1usize)
        }
    } else if let Some(ref path) = cli.seed_result {
        let raw = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("Cannot read seed file {}: {e}", path.display()));
        let rows: Vec<Value> = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("Cannot parse seed file: {e}"));
        (rows, 1usize)
    } else {
        // Default: use built-in seed.
        (load_builtin_seed(), 1usize)
    };

    // ── Run the walkthrough ─────────────────────────────────────────────────
    match cli.store.as_str() {
        "mem" => {
            let mut store = MemStore::with_defaults();
            let result = run_default_script(seed_rows, requery_count, &mut store, "mem")
                .unwrap_or_else(|e| {
                    eprintln!("ERROR: {e}");
                    std::process::exit(1);
                });
            write_outputs(&cli.out, &result);
        }
        #[cfg(feature = "duckdb")]
        "duckdb" => {
            use mqo_duckdb_handle_store::DuckStore;
            let mut store = DuckStore::open_in_memory().unwrap_or_else(|e| {
                eprintln!("ERROR: cannot open DuckDB store: {e}");
                std::process::exit(1);
            });
            let result = run_default_script(seed_rows, requery_count, &mut store, "duckdb")
                .unwrap_or_else(|e| {
                    eprintln!("ERROR: {e}");
                    std::process::exit(1);
                });
            write_outputs(&cli.out, &result);
        }
        other => {
            eprintln!("ERROR: unknown store backend '{other}' (valid: mem, duckdb)");
            std::process::exit(1);
        }
    }
}

fn load_builtin_seed() -> Vec<Value> {
    // Embedded fixture so the binary works standalone.
    let raw = include_str!("../fixtures/seed_result.json");
    serde_json::from_str(raw).expect("built-in seed is valid JSON")
}

fn write_outputs(out_dir: &PathBuf, result: &mqo_handle_walkthrough::walkthrough::WalkthroughResult) {
    fs::create_dir_all(out_dir).unwrap_or_else(|e| {
        eprintln!("WARN: cannot create output dir: {e}");
    });

    // Write transcript.
    let transcript_path = out_dir.join("walkthrough.json");
    let transcript_json = serde_json::to_string_pretty(&result.transcript)
        .expect("transcript serialisation failed");
    fs::write(&transcript_path, &transcript_json)
        .unwrap_or_else(|e| eprintln!("WARN: cannot write transcript: {e}"));
    println!("Transcript written to: {}", transcript_path.display());

    // Write Vega-Lite spec.
    let chart_path = out_dir.join("chart.vg.json");
    let chart_json = serde_json::to_string_pretty(&result.vega_lite)
        .expect("chart serialisation failed");
    fs::write(&chart_path, &chart_json)
        .unwrap_or_else(|e| eprintln!("WARN: cannot write chart: {e}"));
    println!("Vega-Lite spec written to: {}", chart_path.display());

    // Print summary.
    println!(
        "requery_count={} store={} total_handles={}",
        result.transcript.header.requery_count,
        result.transcript.header.store_backend,
        result.transcript.header.total_handles,
    );
}
