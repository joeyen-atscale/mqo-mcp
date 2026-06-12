//! `mqo-route` — route a `BoundMqo` to DAX, MDX, or SQL.
//!
//! Usage:
//!   `mqo-route --bound <bound_mqo.json> --stats <level_cardinalities.json>`
//!              `[--row-threshold <N>]`
//!
//! Outputs a JSON routing decision to stdout.
//!
//! Exit codes:
//!   0 — routing decision emitted
//!   2 — I/O or parse error

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use clap::Parser;
use mqo_backend_router::{route, CatalogContext, StatBundle};
use std::path::PathBuf;
use std::process;

const DEFAULT_ROW_THRESHOLD: u64 = 50_000;

#[derive(Parser, Debug)]
#[command(
    name = "mqo-route",
    about = "Route a BoundMqo to DAX, MDX, or SQL based on shape and cardinality"
)]
struct Args {
    /// Path to the `BoundMqo` JSON file (output of mqo-bind).
    #[arg(long)]
    bound: PathBuf,

    /// Path to level-cardinality stats JSON file.
    #[arg(long)]
    stats: PathBuf,

    /// Row threshold above which SQL streaming is chosen. Default: 50000.
    #[arg(long, default_value_t = DEFAULT_ROW_THRESHOLD)]
    row_threshold: u64,

    /// Path to the catalog snapshot JSON (output of mqo-bind's companion build
    /// step). When provided, the SQL projection uses fully-qualified FROM
    /// (`"catalog"."schema"."model"`), display labels for column names, and
    /// `SUM("Label") AS "slug"` for measures.
    #[arg(long)]
    catalog: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();

    // Load BoundMqo
    let bound_text = std::fs::read_to_string(&args.bound).unwrap_or_else(|e| {
        eprintln!("mqo-route: cannot read --bound file: {e}");
        process::exit(2);
    });
    let bound: mqo_spec::BoundMqo = serde_json::from_str(&bound_text).unwrap_or_else(|e| {
        eprintln!("mqo-route: --bound file is not valid BoundMqo JSON: {e}");
        process::exit(2);
    });

    // Load stats bundle
    let stats_text = std::fs::read_to_string(&args.stats).unwrap_or_else(|e| {
        eprintln!("mqo-route: cannot read --stats file: {e}");
        process::exit(2);
    });
    let stats: StatBundle = serde_json::from_str(&stats_text).unwrap_or_else(|e| {
        eprintln!("mqo-route: --stats file is not valid StatBundle JSON: {e}");
        process::exit(2);
    });

    // Load optional catalog context for fully-qualified SQL generation.
    let catalog_ctx = args.catalog.as_deref().map(|path| {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("mqo-route: cannot read --catalog file: {e}");
            process::exit(2);
        });
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("mqo-route: --catalog file is not valid JSON: {e}");
            process::exit(2);
        });
        CatalogContext::from_json(&v)
    });

    match route(&bound, &stats, args.row_threshold, catalog_ctx.as_ref()) {
        Ok(decision) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&decision).expect("serialize")
            );
            process::exit(0);
        }
        Err(e) => {
            eprintln!("mqo-route: routing error: {e}");
            process::exit(2);
        }
    }
}
