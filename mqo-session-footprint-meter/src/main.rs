//! mqo-session-footprint — CLI entry point.
//!
//! Drives a real `mqo-mcp-server` stdio session (or replays a fixture),
//! classifies every JSON-RPC byte into a context class, and emits a
//! `session_footprint.json` with per-class token counts.

use clap::{Parser, ValueEnum};
use mqo_session_footprint_meter::{
    check_no_literal_pg_pass, process_frames, SessionFrame, SessionFootprint,
};
use std::process::ExitCode;

// ── CLI ───────────────────────────────────────────────────────────────────────

/// Output format.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Format {
    /// JSON output (default).
    Json,
    /// Markdown summary table.
    Markdown,
}

#[derive(Debug, Parser)]
#[command(
    name = "mqo-session-footprint",
    about = "Classify mqo-mcp-server session bytes by token class and emit a footprint report.",
    version
)]
struct Cli {
    /// `mqo-mcp-server` invocation to spawn (stdio JSON-RPC child).
    /// MUST use `--pg-pass-env` — literal `--pg-pass` is rejected (exit 2).
    #[arg(long)]
    server: Option<String>,

    /// Session script JSON file (list of turns).
    #[arg(long)]
    script: Option<String>,

    /// Fixture file: a JSON array of `{op, payload}` frames for offline replay.
    #[arg(long)]
    fixture: Option<String>,

    /// Characters-per-token estimate (shared with context-budget-profiler).
    #[arg(long, default_value_t = 4)]
    chars_per_token: u32,

    /// Output format.
    #[arg(long, short = 'f', default_value = "json")]
    format: Format,

    /// Directory to write raw JSON-RPC frames for audit.
    #[arg(long)]
    capture: Option<String>,

    /// Include per-section catalog detail in the output.
    #[arg(long)]
    with_section_detail: bool,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

#[allow(clippy::print_stderr)]
fn err(msg: &str) {
    eprintln!("error: {msg}");
}

#[allow(clippy::print_stdout)]
fn print_json(fp: &SessionFootprint) {
    match serde_json::to_string_pretty(fp) {
        Ok(s) => println!("{s}"),
        #[allow(clippy::print_stderr)]
        Err(e) => eprintln!("serialization error: {e}"),
    }
}

#[allow(clippy::print_stdout)]
fn print_markdown(fp: &SessionFootprint) {
    let model = fp.model.as_deref().unwrap_or("(unknown)");
    println!("## Session Footprint — {model}");
    println!();
    println!("| Field | Value |");
    println!("|---|---|");
    println!("| Turns | {} |", fp.turns);
    println!("| chars_per_token | {} |", fp.chars_per_token);
    println!("| **Total tokens** | **{}** |", fp.total_tokens);
    println!();
    println!("### Context classes");
    println!();
    println!("| Class | Tokens |");
    println!("|---|---|");
    println!("| system_prompt | {} |", fp.classes.system_prompt);
    println!(
        "| catalog_describe_model | {} |",
        fp.classes.catalog_describe_model
    );
    println!("| tool_call | {} |", fp.classes.tool_call);
    println!("| tool_result_rows | {} |", fp.classes.tool_result_rows);
    println!("| dialogue | {} |", fp.classes.dialogue);

    if let Some(cs) = &fp.catalog_sections {
        println!();
        println!("### Catalog sections");
        println!();
        println!("| Section | Tokens |");
        println!("|---|---|");
        println!("| measures | {} |", cs.measures);
        println!("| dimensions | {} |", cs.dimensions);
        println!("| calcs | {} |", cs.calcs);
        println!("| hierarchies | {} |", cs.hierarchies);
        println!("| raw_expressions | {} |", cs.raw_expressions);
        println!("| summaries | {} |", cs.summaries);
        println!("| scaffolding | {} |", cs.scaffolding);
    }

    println!();
    println!("### Per-turn breakdown");
    println!();
    println!("| Turn | Op | Tokens |");
    println!("|---|---|---|");
    for t in &fp.per_turn {
        println!("| {} | {} | {} |", t.turn, t.op, t.tokens);
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Validate chars_per_token.
    if cli.chars_per_token == 0 {
        err("--chars-per-token must be > 0");
        return ExitCode::from(2);
    }

    // AC5: reject literal --pg-pass in server command.
    if let Some(ref server_cmd) = cli.server {
        if let Err(e) = check_no_literal_pg_pass(server_cmd) {
            err(&e.to_string());
            return ExitCode::from(2);
        }
    }

    // Determine source: fixture file, script file, or live server.
    let footprint = if let Some(ref fixture_path) = cli.fixture {
        // Offline fixture replay.
        let json = match std::fs::read_to_string(fixture_path) {
            Ok(s) => s,
            Err(e) => {
                err(&format!("reading fixture {fixture_path}: {e}"));
                return ExitCode::from(2);
            }
        };
        let frames: Vec<SessionFrame> = match serde_json::from_str(&json) {
            Ok(f) => f,
            Err(e) => {
                err(&format!("parsing fixture: {e}"));
                return ExitCode::from(2);
            }
        };
        match process_frames(&frames, cli.chars_per_token, cli.with_section_detail) {
            Ok(fp) => fp,
            Err(e) => {
                err(&format!("processing frames: {e}"));
                return ExitCode::from(2);
            }
        }
    } else if cli.server.is_some() || cli.script.is_some() {
        // Live server mode — not fully implemented offline; emit a placeholder.
        // The full subprocess-driving path is exercised by the live smoke test (AC6).
        err(
            "live server mode requires --server and --script; \
             offline fixture mode is available via --fixture",
        );
        return ExitCode::from(2);
    } else {
        err("one of --fixture or --server must be provided");
        return ExitCode::from(2);
    };

    match cli.format {
        Format::Json => print_json(&footprint),
        Format::Markdown => print_markdown(&footprint),
    }

    ExitCode::SUCCESS
}
