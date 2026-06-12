//! `mqo-chart-recommender` binary — read a `result-profile.v1` JSON file and
//! print a `chart-recommendation.v1`.
//!
//! Usage:
//! ```text
//! mqo-chart-recommender --profile <file> [--format json|human]
//! ```

#![allow(clippy::print_stdout)] // intentional: this is the CLI output path
#![allow(clippy::print_stderr)] // intentional: error reporting to stderr

use std::{fs, path::PathBuf};

#[cfg(feature = "cli")]
use clap::{Parser, ValueEnum};

use mqo_chart_recommender::recommend;

#[cfg(feature = "cli")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Json,
    Human,
}

#[cfg(feature = "cli")]
#[derive(Debug, Parser)]
#[command(
    name = "mqo-chart-recommender",
    about = "Recommend a chart type from a result-profile.v1 JSON file",
    version
)]
struct Args {
    /// Path to the result-profile.v1 JSON file.
    #[arg(short, long)]
    profile: PathBuf,

    /// Output format: json (default) or human-readable.
    #[arg(short, long, value_enum, default_value = "json")]
    format: Format,
}

#[cfg(feature = "cli")]
fn main() {
    let args = Args::parse();

    let raw = fs::read_to_string(&args.profile)
        .unwrap_or_else(|e| die(&format!("cannot read {:?}: {e}", args.profile)));

    let profile: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| die(&format!("invalid JSON: {e}")));

    let rec = recommend(&profile).unwrap_or_else(|e| die(&format!("recommend error: {e}")));

    match args.format {
        Format::Json => {
            let out = serde_json::to_string_pretty(&rec)
                .unwrap_or_else(|e| die(&format!("serialisation error: {e}")));
            println!("{out}");
        }
        Format::Human => {
            println!("Mark:      {:?}", rec.mark);
            println!("Rationale: {}", rec.rationale);
            if let Some(x) = &rec.encoding.x {
                println!("x:         {} ({})", x.field, x.data_type);
            }
            if let Some(y) = &rec.encoding.y {
                println!("y:         {} ({})", y.field, y.data_type);
            }
            if let Some(c) = &rec.encoding.color {
                println!("color:     {} ({})", c.field, c.data_type);
            }
            if !rec.alternatives.is_empty() {
                println!("Alternatives:");
                for alt in &rec.alternatives {
                    println!("  {:?}: {}", alt.mark, alt.reason);
                }
            }
        }
    }
}

/// Print an error to stderr and exit 1.
fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(1);
}

#[cfg(not(feature = "cli"))]
fn main() {
    eprintln!("Binary requires the `cli` feature; rebuild with --features cli");
    std::process::exit(1);
}
