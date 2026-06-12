//! `mqo-from-sql` — reverse-compile AtScale SQL projections into MQO JSON.
//!
//! Usage:
//!   mqo-from-sql [OPTIONS] [SQL]
//!
//! Exit codes:
//!   0  — success; stdout is MQO JSON
//!   1  — one or more parse/resolve errors (batch: partial success still exits 1)
//!   2  — usage error (bad flags)

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use mqo_from_sql_lib::error;
use mqo_from_sql_lib::mqo_builder;
use mqo_from_sql_lib::parser;

use std::io::{BufRead, Write};
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use mqo_catalog_binder::catalog::CatalogSnapshot;

use error::MqoFromSqlError;

/// Reverse-compile AtScale SQL projections into MQO JSON.
#[derive(Parser, Debug)]
#[command(
    name = "mqo-from-sql",
    about = "Reverse-compile AtScale SQL projections into MQO JSON",
    version
)]
struct Args {
    /// Inline SQL string to compile. Mutually exclusive with --batch.
    #[arg(value_name = "SQL")]
    sql: Option<String>,

    /// CatalogSnapshot JSON file.
    #[arg(long, value_name = "FILE")]
    catalog: Option<PathBuf>,

    /// JSONL input file (one SQL per line). Mutually exclusive with SQL positional arg.
    #[arg(long, value_name = "FILE")]
    batch: Option<PathBuf>,

    /// Output file (default: stdout).
    #[arg(long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Output format.
    #[arg(long, value_enum, default_value = "json")]
    format: OutputFormat,

    /// Environment variable name containing the password for live catalog fetch.
    /// The actual password must NOT be passed directly on the command line.
    #[arg(long, value_name = "VARNAME")]
    pg_pass_env: Option<String>,
}

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    Json,
    Jsonl,
}

fn main() {
    let args = Args::parse();

    // Load catalog snapshot if provided
    let snapshot: Option<CatalogSnapshot> = match &args.catalog {
        None => None,
        Some(path) => {
            let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
                eprintln!("mqo-from-sql: cannot read --catalog file: {e}");
                std::process::exit(2);
            });
            let snap: CatalogSnapshot = serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("mqo-from-sql: --catalog file is not valid JSON: {e}");
                std::process::exit(2);
            });
            Some(snap)
        }
    };

    // Resolve output writer
    let mut out: Box<dyn Write> = match &args.output {
        None => Box::new(std::io::stdout()),
        Some(path) => Box::new(std::fs::File::create(path).unwrap_or_else(|e| {
            eprintln!("mqo-from-sql: cannot create output file: {e}");
            std::process::exit(2);
        })),
    };

    let exit_code = if let Some(batch_path) = &args.batch {
        run_batch(batch_path, snapshot.as_ref(), &args.format, &mut out)
    } else {
        let sql = args.sql.as_deref().unwrap_or_else(|| {
            eprintln!("mqo-from-sql: provide SQL as positional argument or use --batch");
            std::process::exit(2);
        });
        run_single(sql, snapshot.as_ref(), &args.format, &mut out)
    };

    std::process::exit(exit_code);
}

fn run_single(
    sql: &str,
    snapshot: Option<&CatalogSnapshot>,
    format: &OutputFormat,
    out: &mut dyn Write,
) -> i32 {
    match compile_sql(sql, snapshot) {
        Ok(bound) => {
            let json = to_output_string(&bound, format);
            writeln!(out, "{json}").ok();
            0
        }
        Err(e) => {
            eprintln!("mqo-from-sql: error: {e}");
            1
        }
    }
}

fn to_output_string(bound: &mqo_spec::BoundMqo, format: &OutputFormat) -> String {
    match format {
        OutputFormat::Json => {
            serde_json::to_string_pretty(bound).expect("BoundMqo serializes")
        }
        OutputFormat::Jsonl => {
            serde_json::to_string(bound).expect("BoundMqo serializes")
        }
    }
}

fn run_batch(
    path: &PathBuf,
    snapshot: Option<&CatalogSnapshot>,
    format: &OutputFormat,
    out: &mut dyn Write,
) -> i32 {
    let file = std::fs::File::open(path).unwrap_or_else(|e| {
        eprintln!("mqo-from-sql: cannot open --batch file: {e}");
        std::process::exit(2);
    });

    let reader = std::io::BufReader::new(file);
    let mut had_error = false;

    for (line_no, line_result) in reader.lines().enumerate() {
        let line = match line_result {
            Ok(l) => l,
            Err(e) => {
                let record = serde_json::json!({
                    "line": line_no + 1,
                    "error": format!("I/O error reading line: {e}"),
                    "ok": false
                });
                writeln!(out, "{record}").ok();
                had_error = true;
                continue;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match compile_sql(trimmed, snapshot) {
            Ok(bound) => {
                let json = to_output_string(&bound, format);
                writeln!(out, "{json}").ok();
            }
            Err(e) => {
                let record = serde_json::json!({
                    "line": line_no + 1,
                    "sql": trimmed,
                    "error": format!("{e}"),
                    "ok": false
                });
                writeln!(out, "{record}").ok();
                eprintln!("mqo-from-sql: batch line {}: {e}", line_no + 1);
                had_error = true;
            }
        }
    }

    if had_error { 1 } else { 0 }
}

fn compile_sql(
    sql: &str,
    snapshot: Option<&CatalogSnapshot>,
) -> Result<mqo_spec::BoundMqo, MqoFromSqlError> {
    let parsed = parser::parse_sql(sql)?;

    let snap = snapshot.ok_or_else(|| {
        MqoFromSqlError::Resolve(error::ResolveError::UnknownName(
            "no --catalog provided; cannot resolve column names".to_string(),
        ))
    })?;

    mqo_builder::build_bound_mqo(&parsed, snap)
}
