//! `mqoguard-regress` — CI gate that scores mcp-tuner trajectories.jsonl
//! against a pinned baseline and exits non-zero on regression.
//!
//! Exit codes:
//!   0 — all gated modes meet their floor
//!   1 — at least one mode below its floor
//!   2 — malformed input (bad JSON, missing required fields, file not found)
//!
//! Scoring semantics match `runner/score_path_correctness.py` for
//! `path_incompatible` tasks and dim/calc path checks (FR3).

use clap::{Parser, ValueEnum};
use mqoguard_regression_harness::report;
use mqoguard_regression_harness::types::{Baseline, Corpus, TrajectoryRecord};
use std::path::PathBuf;
use std::process::ExitCode;

/// CLI for mqoguard regression gating.
#[derive(Parser, Debug)]
#[command(
    name = "mqoguard-regress",
    about = "Score mcp-tuner trajectories.jsonl against a pinned baseline; exit 1 on regression.",
    version
)]
struct Cli {
    /// Path to the corpus JSON (e.g. `tpcds_failure_modes_100_nonprod.json`).
    #[arg(long)]
    tasks: PathBuf,

    /// Path to the trajectories JSONL.
    #[arg(long)]
    records: PathBuf,

    /// Path to the baseline JSON with per-mode floors.
    #[arg(long)]
    baseline: PathBuf,

    /// Output format.
    #[arg(long, default_value = "human")]
    format: OutputFormat,

    /// Tolerance subtracted from each floor before gating (absorbs k=4 sampling
    /// variance). Default 0.0 means exact floor.
    #[arg(long, default_value_t = 0.0)]
    tolerance: f64,
}

/// Output format.
#[derive(ValueEnum, Clone, Debug, PartialEq, Eq)]
enum OutputFormat {
    /// Human-readable table.
    Human,
    /// Machine-readable JSON report.
    Json,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Load all inputs, returning exit 2 on any parse/IO error.
    let corpus = match load_corpus(&cli.tasks) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ERROR loading tasks {}: {e}", cli.tasks.display());
            return ExitCode::from(2);
        }
    };

    let records = match load_records(&cli.records) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("ERROR loading records {}: {e}", cli.records.display());
            return ExitCode::from(2);
        }
    };

    let baseline = match load_baseline(&cli.baseline) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("ERROR loading baseline {}: {e}", cli.baseline.display());
            return ExitCode::from(2);
        }
    };

    // Score.
    let rpt = report::build_report(&corpus, &records, &baseline, cli.tolerance);

    // Emit.
    match cli.format {
        OutputFormat::Human => report::print_human(&rpt),
        OutputFormat::Json => {
            match serde_json::to_string_pretty(&rpt) {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("ERROR serialising report: {e}");
                    return ExitCode::from(2);
                }
            }
        }
    }

    // Exit code.
    if rpt.overall.any_below_floor {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

// ---- file loaders ----

fn load_corpus(path: &PathBuf) -> Result<Corpus, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {e}"))
}

fn load_records(path: &PathBuf) -> Result<Vec<TrajectoryRecord>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let rec: TrajectoryRecord = serde_json::from_str(trimmed)
            .map_err(|e| format!("line {}: JSON parse error: {e}", i + 1))?;
        out.push(rec);
    }
    Ok(out)
}

fn load_baseline(path: &PathBuf) -> Result<Baseline, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {e}"))
}
