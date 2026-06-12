//! `mqo-result-profiler` binary — profile a `query_multidimensional` response.
//!
//! Usage: `mqo-result-profiler --response <file> --catalog <file> [--format json|human]`

#![forbid(unsafe_code)]

use clap::{Parser, ValueEnum};
use mqo_result_profiler::{profile, ColumnProfile};
use std::path::PathBuf;

#[derive(Debug, Clone, ValueEnum)]
enum Format {
    Json,
    Human,
}

#[derive(Parser, Debug)]
#[command(name = "mqo-result-profiler", about = "Profile a query_multidimensional response")]
struct Args {
    /// Path to the response JSON file (`query_multidimensional` structuredContent or rows+bound).
    #[arg(long)]
    response: PathBuf,

    /// Path to the catalog JSON file.
    #[arg(long)]
    catalog: PathBuf,

    /// Output format: json (default) or human.
    #[arg(long, default_value = "json")]
    format: Format,
}

fn main() -> std::process::ExitCode {
    let args = Args::parse();

    let response_text = match std::fs::read_to_string(&args.response) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error reading response file: {e}");
            return std::process::ExitCode::from(2);
        }
    };
    let catalog_text = match std::fs::read_to_string(&args.catalog) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error reading catalog file: {e}");
            return std::process::ExitCode::from(2);
        }
    };

    let response: serde_json::Value = match serde_json::from_str(&response_text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response JSON: {e}");
            return std::process::ExitCode::from(2);
        }
    };
    let catalog: serde_json::Value = match serde_json::from_str(&catalog_text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing catalog JSON: {e}");
            return std::process::ExitCode::from(2);
        }
    };

    let result = match profile(&response, &catalog) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("profiling error: {e}");
            return std::process::ExitCode::from(1);
        }
    };

    match args.format {
        Format::Json => {
            match serde_json::to_string_pretty(&result) {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("serialization error: {e}");
                    return std::process::ExitCode::from(1);
                }
            }
        }
        Format::Human => {
            println!(
                "ResultProfile: {} rows, {} measures, {} dimensions",
                result.row_count, result.measure_count, result.dimension_count
            );
            println!("{:<20} {:<10} {:<14} {:>12} {:>10} {:<12} {:<8}",
                "name", "role", "data_type", "cardinality", "null_rate", "range", "flags");
            println!("{}", "-".repeat(90));
            for col in &result.columns {
                print_human_col(col);
            }
        }
    }

    std::process::ExitCode::SUCCESS
}

fn print_human_col(col: &ColumnProfile) {
    let role = format!("{:?}", col.role);
    let dt = format!("{:?}", col.data_type);
    let range = col
        .measure_range
        .map_or_else(|| "—".to_owned(), |(lo, hi)| format!("{lo:.1}–{hi:.1}"));
    let flags = [
        if col.is_calc { "calc" } else { "" },
        if col.semi_additive { "semi_add" } else { "" },
    ]
    .iter()
    .filter(|s| !s.is_empty())
    .copied()
    .collect::<Vec<_>>()
    .join(",");
    println!(
        "{:<20} {:<10} {:<14} {:>12} {:>10.3} {:<12} {:<8}",
        col.name, role, dt, col.cardinality, col.null_rate, range, flags
    );
}
