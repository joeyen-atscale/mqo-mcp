//! `dh-mcp-server` — the dataset-handle MCP server over JSON-RPC 2.0 on stdio.
//!
//! Usage:
//!   dh-mcp-server --catalog <snapshot.json> [--stats <stats.json>]
//!                 [--release-dir <dir>] [--row-threshold <N>]
//!                 [--sample-cap <N>] [--max-bytes <N>]
//!
//! Reads newline-delimited JSON-RPC requests on stdin and writes
//! newline-delimited responses on stdout (the MCP stdio transport).

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use clap::Parser;
use dh_mcp_server::{Server, ToolPaths, DEFAULT_MAX_TOTAL_BYTES, DEFAULT_SAMPLE_CAP};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process;

const DEFAULT_ROW_THRESHOLD: u64 = 50_000;

#[derive(Parser, Debug)]
#[command(
    name = "dh-mcp-server",
    about = "Dataset-handle MCP server: query_multidimensional → {summary, handle, capabilities} + dataset_* tools"
)]
struct Args {
    /// Path to the recorded catalog snapshot JSON (grounds the binder).
    #[arg(long)]
    catalog: PathBuf,

    /// Path to the router stats JSON (level cardinalities + shape flags).
    #[arg(long)]
    stats: Option<PathBuf>,

    /// Directory containing the fleet release binaries (mqo-bind, mqo-route,
    /// mqo-dax, mqo-mdx). Falls back to ~/.local/bin then PATH.
    #[arg(long)]
    release_dir: Option<PathBuf>,

    /// Router row threshold above which the SQL extract path is chosen.
    #[arg(long, default_value_t = DEFAULT_ROW_THRESHOLD)]
    row_threshold: u64,

    /// Maximum number of sample rows in any returned summary.
    #[arg(long, default_value_t = DEFAULT_SAMPLE_CAP)]
    sample_cap: usize,

    /// Maximum total bytes held by the in-memory store (0 = unlimited).
    #[arg(long, default_value_t = DEFAULT_MAX_TOTAL_BYTES)]
    max_bytes: usize,
}

fn main() {
    let args = Args::parse();

    let catalog: Value = read_json(&args.catalog).unwrap_or_else(|e| {
        eprintln!("dh-mcp-server: {e}");
        process::exit(2);
    });

    let stats: Value = match &args.stats {
        Some(p) => read_json(p).unwrap_or_else(|e| {
            eprintln!("dh-mcp-server: {e}");
            process::exit(2);
        }),
        None => serde_json::json!({ "level_cardinalities": {}, "shape_flags": {} }),
    };

    let tools = ToolPaths::resolve(args.release_dir.as_deref());

    let mut server = Server::new(
        catalog,
        stats,
        tools,
        args.row_threshold,
        args.max_bytes,
        args.sample_cap,
    );

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let err = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32700, "message": format!("parse error: {e}") }
                });
                let _ = writeln!(out, "{err}");
                let _ = out.flush();
                continue;
            }
        };

        if let Some(resp) = server.handle(&req) {
            if writeln!(out, "{resp}").is_err() {
                break;
            }
            let _ = out.flush();
        }
    }
}

fn read_json(path: &std::path::Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))
}
