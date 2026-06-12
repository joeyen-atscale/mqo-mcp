//! # mqo-bench
//!
//! MQO vs text-to-SQL benchmark CLI.
//!
//! Usage:
//! ```text
//! mqo-bench --tasks tasks.json \
//!           --grader path/to/grader \
//!           [--fixture-a fixture_a.json] \
//!           [--fixture-b fixture_b.json] \
//!           [--output-json report.json] \
//!           [--output-md report.md]
//! ```

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::similar_names,
    clippy::cast_precision_loss,
    clippy::doc_markdown,
    clippy::must_use_candidate
)]

use clap::Parser;
use mqo_vs_sql_bench::{report, runner, types};
use std::process;

/// MQO vs text-to-SQL benchmark CLI.
#[derive(Parser, Debug)]
#[command(
    name = "mqo-bench",
    version,
    about = "MQO vs text-to-SQL benchmark — arm A (SQL/run_query) vs arm B (MQO/query_multidimensional)"
)]
struct Cli {
    /// Path to the tasks JSON file (array of task objects).
    #[arg(long)]
    tasks: String,

    /// External grader command (must accept a JSON file path and emit a GraderVerdict on stdout).
    #[arg(long, default_value = "slai-text-to-sql-accuracy-bench")]
    grader: String,

    /// Fixture file for arm A outputs (JSON array of FixtureRecord). Skips live cluster for arm A.
    #[arg(long)]
    fixture_a: Option<String>,

    /// Fixture file for arm B outputs (JSON array of FixtureRecord). Skips live cluster for arm B.
    #[arg(long)]
    fixture_b: Option<String>,

    /// Write per-question + aggregate JSON report to this path.
    #[arg(long)]
    output_json: Option<String>,

    /// Write Markdown report to this path.
    #[arg(long)]
    output_md: Option<String>,
}

fn main() {
    let cli = Cli::parse();

    // Load tasks.
    let tasks_raw = match std::fs::read_to_string(&cli.tasks) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read tasks file '{}': {e}", cli.tasks);
            process::exit(1);
        }
    };
    let tasks: Vec<types::Task> = match serde_json::from_str(&tasks_raw) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot parse tasks JSON: {e}");
            process::exit(1);
        }
    };

    if tasks.is_empty() {
        eprintln!("error: tasks file is empty");
        process::exit(1);
    }

    eprintln!("mqo-bench: running {} tasks", tasks.len());

    let config = runner::RunConfig {
        grader_cmd: cli.grader.clone(),
        fixture_a: cli.fixture_a.clone(),
        fixture_b: cli.fixture_b.clone(),
    };

    let results = match runner::run(&tasks, &config) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: benchmark run failed: {e}");
            process::exit(1);
        }
    };

    let agg = report::compute_aggregate(&results);

    // Emit JSON report.
    let json_report = serde_json::json!({
        "aggregate": agg,
        "questions": results,
    });
    let json_str = serde_json::to_string_pretty(&json_report)
        .expect("serialization of report cannot fail");

    if let Some(path) = &cli.output_json {
        if let Err(e) = std::fs::write(path, &json_str) {
            eprintln!("error: cannot write JSON report to '{path}': {e}");
            process::exit(1);
        }
        eprintln!("mqo-bench: JSON report written to {path}");
    } else {
        println!("{json_str}");
    }

    // Emit Markdown report.
    let md = report::render_markdown(&results, &agg);
    if let Some(path) = &cli.output_md {
        if let Err(e) = std::fs::write(path, &md) {
            eprintln!("error: cannot write Markdown report to '{path}': {e}");
            process::exit(1);
        }
        eprintln!("mqo-bench: Markdown report written to {path}");
    } else if cli.output_json.is_some() {
        // JSON went to file; print markdown summary to stdout too.
        println!("{md}");
    } else {
        // Neither flag: JSON was already printed above; now print markdown.
        println!("{md}");
    }
}
