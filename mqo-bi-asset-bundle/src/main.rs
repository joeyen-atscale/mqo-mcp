//! `mqo-bi-asset-bundle` CLI — one-shot MCP query response → BI asset bundle.

#![forbid(unsafe_code)]
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use mqo_bi_asset_bundle::build_asset;
use serde_json::json;

/// One-shot BI asset bundle: profiler → recommender → emitter.
///
/// Reads a `query_multidimensional` response and a catalog JSON, and prints a
/// complete `bi-asset.v1` bundle to stdout.
#[derive(Parser, Debug)]
#[command(name = "mqo-bi-asset-bundle", version, about)]
struct Args {
    /// Path to the `query_multidimensional` response JSON file.
    #[arg(long)]
    response: PathBuf,

    /// Path to the catalog JSON file.
    #[arg(long)]
    catalog: PathBuf,

    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Json)]
    format: Format,
}

/// Output format.
#[derive(Clone, Debug, ValueEnum)]
enum Format {
    /// Emit the full `bi-asset.v1` JSON object (default).
    Json,
    /// Emit a human-readable rendering (title, description, caveats note).
    Human,
}

fn read_json_file(path: &PathBuf, label: &str) -> Result<serde_json::Value, (String, String)> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        let err = json!({
            "error": "io_error",
            "message": format!("cannot read {label} file: {e}"),
            "path": path.display().to_string(),
        });
        (err.to_string(), String::new())
    })?;
    serde_json::from_str(&text).map_err(|e| {
        let err = json!({
            "error": "json_parse_error",
            "message": format!("{label} file is not valid JSON: {e}"),
        });
        (err.to_string(), String::new())
    })
}

fn print_human(asset: &mqo_bi_asset_bundle::BiAsset) {
    println!("Title:       {}", asset.title);
    println!("Description: {}", asset.description);
    if asset.caveats.is_empty() {
        println!("Caveats:     (none)");
    } else {
        for (i, caveat) in asset.caveats.iter().enumerate() {
            println!("Caveat {}:    {caveat}", i + 1);
        }
    }
    println!("Spec:        embedded ({} rows)", asset.profile_summary.row_count);
}

fn main() -> ExitCode {
    let args = Args::parse();

    let response = match read_json_file(&args.response, "response") {
        Ok(v) => v,
        Err((msg, _)) => { eprintln!("{msg}"); return ExitCode::from(1); }
    };

    let catalog = match read_json_file(&args.catalog, "catalog") {
        Ok(v) => v,
        Err((msg, _)) => { eprintln!("{msg}"); return ExitCode::from(1); }
    };

    match build_asset(&response, &catalog) {
        Ok(asset) => {
            match args.format {
                Format::Json => {
                    match serde_json::to_string_pretty(&asset) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            let err = json!({"error": "serialization_error", "message": format!("{e}")});
                            eprintln!("{err}");
                            return ExitCode::from(1);
                        }
                    }
                }
                Format::Human => print_human(&asset),
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            let err = json!({"error": "bundle_error", "message": e.to_string()});
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}
