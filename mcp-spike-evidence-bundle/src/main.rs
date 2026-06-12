//! `mcp-spike-evidence` — CLI entry point.
//!
//! Ingests four sibling artifact JSON files plus a ticket-map, scores each
//! ticket AC, and emits `spike_evidence.json` plus an optional Markdown brief.

use clap::{Parser, ValueEnum};
use mcp_spike_evidence_bundle::{parse_json_file, parse_ticket_map, render_markdown, run_bundle, ArtifactMap};
use std::process::ExitCode;

// ── CLI ───────────────────────────────────────────────────────────────────────

/// Output format.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Format {
    /// JSON output (default). Writes `spike_evidence.json` to stdout.
    Json,
    /// Markdown hand-back brief to stdout.
    Markdown,
}

/// `mcp-spike-evidence` — map grooming spike ACs to produced artifacts and verdicts.
#[derive(Debug, Parser)]
#[command(
    name = "mcp-spike-evidence",
    about = "Ingest spike artifacts, score each AC, and emit spike_evidence.json.",
    version
)]
struct Cli {
    /// `session_footprint.json` from mqo-session-footprint-meter.
    #[arg(long)]
    footprint: Option<String>,

    /// `bench_report.json` from mqo-paramq-bench.
    #[arg(long)]
    paramq: Option<String>,

    /// walkthrough.json from mqo-handle-walkthrough.
    #[arg(long)]
    walkthrough: Option<String>,

    /// demo.json from mqo-duckdb-handle-store (handle put/get round-trip record).
    #[arg(long)]
    handle_demo: Option<String>,

    /// tickets.json — the AC inventory per ticket (checked-in ticket-map).
    #[arg(long, required = true)]
    ticket_map: String,

    /// Output format.
    #[arg(long, short = 'f', default_value = "json")]
    format: Format,

    /// Write `spike_evidence.json` to this path (only with --format json).
    #[arg(long)]
    output: Option<String>,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

#[allow(clippy::print_stderr)]
fn err(msg: &str) {
    eprintln!("error: {msg}");
}

#[allow(clippy::print_stdout)]
fn print_output(s: &str) {
    println!("{s}");
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Parse ticket-map (required).
    let ticket_map = match parse_ticket_map(&cli.ticket_map) {
        Ok(tm) => tm,
        Err(e) => {
            err(&e);
            return ExitCode::from(2);
        }
    };

    // Parse optional artifact files.
    let mut artifacts = ArtifactMap::new();

    let artifact_args: &[(&str, &Option<String>)] = &[
        ("footprint", &cli.footprint),
        ("paramq", &cli.paramq),
        ("walkthrough", &cli.walkthrough),
        ("handle_demo", &cli.handle_demo),
    ];

    for (key, path_opt) in artifact_args {
        if let Some(path) = path_opt {
            match parse_json_file(path) {
                Ok(val) => {
                    artifacts.insert((*key).to_owned(), val);
                }
                Err(e) => {
                    err(&e);
                    return ExitCode::from(2);
                }
            }
        }
        // Missing optional artifact: ACs that depend on it will get gap/skip-needs-live.
    }

    // Run the bundle.
    let evidence = run_bundle(&ticket_map, &artifacts);

    // Emit output.
    match cli.format {
        Format::Json => {
            let json_str = match serde_json::to_string_pretty(&evidence) {
                Ok(s) => s,
                Err(e) => {
                    err(&format!("serializing evidence: {e}"));
                    return ExitCode::from(2);
                }
            };

            if let Some(out_path) = &cli.output {
                if let Err(e) = std::fs::write(out_path, &json_str) {
                    err(&format!("writing '{out_path}': {e}"));
                    return ExitCode::from(2);
                }
            } else {
                print_output(&json_str);
            }
        }
        Format::Markdown => {
            let md = render_markdown(&evidence);
            print_output(&md);
        }
    }

    ExitCode::SUCCESS
}
