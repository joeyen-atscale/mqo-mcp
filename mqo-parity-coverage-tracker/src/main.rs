//! `parity-tracker` CLI — thin argument-plumbing + format rendering.
//!
//! All computation lives in `mqo_parity_coverage_tracker` lib.
//!
//! ## Subcommands
//!
//! * `record --input <path|-> --build-id <id> [--history <path>]`
//! * `coverage --build-id <id> --corpus <path> [--history <path>] [--format json|human]`
//! * `delta --build <N> --against <N-1> --corpus <path> [--history <path>] [--format json|human] [--no-fail]`
//!
//! ## Exit codes
//!
//! * 0 — success (or `coverage`/`record` always succeed)
//! * 1 — I/O or parse error
//! * 2 — `delta` newly-broken list is non-empty (unless `--no-fail`)

#![forbid(unsafe_code)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand, ValueEnum};

use mqo_parity_coverage_tracker::{
    corpus::Corpus,
    coverage_summary, delta_summary,
    default_store_path,
    load_history,
    record_run,
    store::{InputReport, LiveHarnessCase, ParityReportInput},
    summary::{CoverageStatus, CoverageSummary, DeltaSummary},
};

// ── CLI definition ─────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
#[command(
    name = "parity-tracker",
    about = "Record and query build-stamped parity verdicts for (measure, backend-pair) coverage.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Json,
    Human,
}

impl Default for Format {
    fn default() -> Self {
        Self::Human
    }
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Ingest a parity result JSON and stamp it with a build id.
    Record {
        /// Path to the input JSON file, or '-' to read from stdin.
        #[arg(long, default_value = "-")]
        input: String,

        /// The server build id to stamp this result with (REQUIRED, never inferred).
        #[arg(long)]
        build_id: String,

        /// Path to the JSONL history store.
        #[arg(long)]
        history: Option<PathBuf>,

        /// Interpret the input as live-harness per-case results (array of objects)
        /// rather than a ParityReport.
        #[arg(long)]
        live_harness: bool,

        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Human)]
        format: Format,
    },

    /// Compute per-(measure, backend-pair) coverage for a build id.
    Coverage {
        /// The build id to compute coverage for.
        #[arg(long)]
        build_id: String,

        /// Path to the parity-corpus.v1 JSON file.
        #[arg(long)]
        corpus: PathBuf,

        /// Path to the JSONL history store.
        #[arg(long)]
        history: Option<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Human)]
        format: Format,

        /// Hint the backend pair when it cannot be inferred from history (e.g. "dax").
        #[arg(long)]
        backend_a: Option<String>,

        /// Second backend in the pair hint (e.g. "sql").
        #[arg(long)]
        backend_b: Option<String>,
    },

    /// Show build-over-build delta: newly-verified, newly-broken, newly-tested.
    Delta {
        /// The candidate (newer) build id.
        #[arg(long)]
        build: String,

        /// The prior build id to compare against.
        #[arg(long)]
        against: String,

        /// Path to the parity-corpus.v1 JSON file.
        #[arg(long)]
        corpus: PathBuf,

        /// Path to the JSONL history store.
        #[arg(long)]
        history: Option<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Human)]
        format: Format,

        /// Do not exit non-zero when newly-broken list is non-empty.
        #[arg(long)]
        no_fail: bool,

        /// Hint the backend pair.
        #[arg(long)]
        backend_a: Option<String>,

        /// Second backend in the pair hint.
        #[arg(long)]
        backend_b: Option<String>,
    },

    /// List all build ids recorded in the history store.
    Regressions {
        /// Path to the JSONL history store.
        #[arg(long)]
        history: Option<PathBuf>,

        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Human)]
        format: Format,
    },
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn resolve_store(opt: Option<PathBuf>) -> PathBuf {
    opt.unwrap_or_else(default_store_path)
}

fn read_input(path: &str) -> Result<String, String> {
    if path == "-" {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("failed to read stdin: {e}"))?;
        Ok(buf)
    } else {
        std::fs::read_to_string(path).map_err(|e| format!("failed to read {path}: {e}"))
    }
}

fn parse_corpus(path: &Path) -> Result<Corpus, String> {
    let s = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read corpus {}: {e}", path.display()))?;
    Corpus::from_json(&s).map_err(|e| format!("failed to parse corpus: {e}"))
}

fn parse_input_report(raw: &str, live_harness: bool, source: &str) -> Result<InputReport, String> {
    if live_harness {
        let cases: Vec<LiveHarnessCase> = serde_json::from_str(raw)
            .map_err(|e| format!("failed to parse live-harness results from {source}: {e}"))?;
        Ok(InputReport::LiveHarnessResults(cases))
    } else {
        let report: ParityReportInput = serde_json::from_str(raw)
            .map_err(|e| format!("failed to parse ParityReport from {source}: {e}"))?;
        Ok(InputReport::ParityReport(report))
    }
}

// ── Rendering ──────────────────────────────────────────────────────────────

fn render_coverage(summary: &CoverageSummary, format: Format) {
    match format {
        Format::Json => {
            println!("{}", serde_json::to_string_pretty(summary).unwrap());
        }
        Format::Human => {
            println!(
                "Coverage for build '{}' [{}, denominator: {} corpus cells ({})]: {:.1}%",
                summary.build_id,
                summary.backend_a,
                summary.denominator,
                summary.corpus_version,
                summary.coverage_pct
            );
            println!(
                "  verified={} mismatch={} never-tested={}",
                summary.verified_count, summary.mismatch_count, summary.never_tested_count
            );
            if !summary.ungrounded_cells.is_empty() {
                println!("  ungrounded (has_grounding=false): {}", summary.ungrounded_cells.len());
            }
            println!();
            println!(
                "  {:<50} {:<12} {:<12} STATUS",
                "MEASURE", "BACKEND_A", "BACKEND_B"
            );
            println!("  {}", "-".repeat(90));
            for cell in &summary.cells {
                let status_str = match cell.status {
                    CoverageStatus::Verified => "verified",
                    CoverageStatus::Mismatch => "MISMATCH",
                    CoverageStatus::NeverTested => "never-tested",
                };
                println!(
                    "  {:<50} {:<12} {:<12} {}{}",
                    cell.measure,
                    cell.backend_a,
                    cell.backend_b,
                    status_str,
                    if cell.ungrounded { " [ungrounded]" } else { "" }
                );
            }
        }
    }
}

fn render_delta(delta: &DeltaSummary, format: Format) {
    match format {
        Format::Json => {
            println!("{}", serde_json::to_string_pretty(delta).unwrap());
        }
        Format::Human => {
            println!(
                "Delta: build '{}' vs '{}' — newly-broken: {}",
                delta.build_n,
                delta.build_prior,
                delta.newly_broken.len()
            );
            if delta.prior_was_absent {
                println!("  WARNING: prior build '{}' has no recorded history; all current verdicts treated as newly-tested.", delta.build_prior);
            }
            if !delta.newly_broken.is_empty() {
                println!("\n  NEWLY BROKEN ({}):", delta.newly_broken.len());
                for m in &delta.newly_broken {
                    println!("    - {}", m);
                }
            }
            if !delta.newly_verified.is_empty() {
                println!("\n  Newly verified ({}):", delta.newly_verified.len());
                for m in &delta.newly_verified {
                    println!("    + {}", m);
                }
            }
            if !delta.newly_tested.is_empty() {
                println!("\n  Newly tested ({}):", delta.newly_tested.len());
                for m in &delta.newly_tested {
                    println!("    ~ {}", m);
                }
            }
            if delta.newly_broken.is_empty()
                && delta.newly_verified.is_empty()
                && delta.newly_tested.is_empty()
            {
                println!("  No changes between recorded builds.");
            }
        }
    }
}

// ── Main ───────────────────────────────────────────────────────────────────

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Record {
            input,
            build_id,
            history,
            live_harness,
            format,
        } => {
            let store_path = resolve_store(history);
            let raw = match read_input(&input) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("ERROR: {e}");
                    process::exit(1);
                }
            };
            let source = if input == "-" { "stdin".to_string() } else { input.clone() };
            let report = match parse_input_report(&raw, live_harness, &source) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("ERROR: {e}");
                    process::exit(1);
                }
            };
            match record_run(&report, &build_id, &source, &store_path) {
                Ok(entry) => {
                    match format {
                        Format::Json => {
                            println!("{}", serde_json::to_string_pretty(&entry).unwrap());
                        }
                        Format::Human => {
                            println!(
                                "Recorded build '{}': {} pair(s), all_skipped={}, digest={}",
                                entry.build_id,
                                entry.pairs.len(),
                                entry.all_skipped,
                                &entry.content_digest[..8]
                            );
                            println!("  store: {}", store_path.display());
                        }
                    }
                }
                Err(e) => {
                    eprintln!("ERROR: failed to record: {e}");
                    process::exit(1);
                }
            }
        }

        Commands::Coverage {
            build_id,
            corpus,
            history,
            format,
            backend_a,
            backend_b,
        } => {
            let store_path = resolve_store(history);
            let hist = match load_history(&store_path) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("ERROR: failed to load history: {e}");
                    process::exit(1);
                }
            };
            let corp = match parse_corpus(&corpus) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("ERROR: {e}");
                    process::exit(1);
                }
            };
            let hint = match (&backend_a, &backend_b) {
                (Some(a), Some(b)) => Some((a.as_str(), b.as_str())),
                _ => None,
            };
            let summary = coverage_summary(&hist, &corp, &build_id, hint);
            render_coverage(&summary, format);
        }

        Commands::Delta {
            build,
            against,
            corpus,
            history,
            format,
            no_fail,
            backend_a,
            backend_b,
        } => {
            let store_path = resolve_store(history);
            let hist = match load_history(&store_path) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("ERROR: failed to load history: {e}");
                    process::exit(1);
                }
            };
            let corp = match parse_corpus(&corpus) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("ERROR: {e}");
                    process::exit(1);
                }
            };
            let hint = match (&backend_a, &backend_b) {
                (Some(a), Some(b)) => Some((a.as_str(), b.as_str())),
                _ => None,
            };
            let delta = delta_summary(&hist, &corp, &build, &against, hint);
            render_delta(&delta, format);

            if !no_fail && !delta.newly_broken.is_empty() {
                process::exit(2);
            }
        }

        Commands::Regressions { history, format } => {
            let store_path = resolve_store(history);
            let hist = match load_history(&store_path) {
                Ok(h) => h,
                Err(e) => {
                    eprintln!("ERROR: failed to load history: {e}");
                    process::exit(1);
                }
            };

            // Collect all build ids in order seen.
            let mut seen = std::collections::HashSet::new();
            let mut builds: Vec<&str> = Vec::new();
            for entry in &hist {
                if seen.insert(entry.build_id.as_str()) {
                    builds.push(&entry.build_id);
                }
            }

            match format {
                Format::Json => {
                    println!("{}", serde_json::to_string_pretty(&builds).unwrap());
                }
                Format::Human => {
                    println!("Recorded builds ({} total):", builds.len());
                    for b in &builds {
                        println!("  {}", b);
                    }
                }
            }
        }
    }
}
