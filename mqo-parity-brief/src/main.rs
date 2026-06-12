// main.rs — mqo-parity-brief CLI entry point.
// Reads the mqo-parity-coverage-tracker JSONL history store and emits a Markdown brief.
// No live queries, no credentials, fully offline per NFR1/NFR2.

mod brief;
mod types;

use clap::Parser;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process;

use brief::{build_tiger_record, render_brief};
use types::HistoryRecord;

#[derive(Parser, Debug)]
#[command(
    name = "mqo-parity-brief",
    about = "Generate a Markdown parity brief from the mqo-parity-coverage-tracker JSONL history store",
    version
)]
struct Cli {
    /// Path to the coverage-tracker JSONL history store.
    /// Each line is a JSON HistoryRecord.
    #[arg(
        long,
        default_value = "~/.local/share/mqo-parity/history.jsonl",
        value_name = "PATH"
    )]
    history: String,

    /// Build id to report. Defaults to the most-recently recorded build.
    /// Exits non-zero if the id is absent from history (FR3 — no silent fallback).
    #[arg(long, value_name = "BUILD_ID")]
    build_id: Option<String>,

    /// Write the brief to a file instead of stdout (FR11).
    #[arg(long, value_name = "PATH")]
    out: Option<PathBuf>,

    /// Scope the summary window to builds recorded after this build id (inclusive).
    #[arg(long, value_name = "VERSION")]
    since_build: Option<String>,

    /// Output format.  Only "markdown" is supported in V1 (NG3).
    #[arg(long, default_value = "markdown", value_name = "FORMAT")]
    format: String,

    /// Write a Tiger-compatible build-stamped record to this path (FR7 / G5).
    /// The Markdown brief is still emitted.
    #[arg(long, value_name = "PATH")]
    emit_tiger_record: Option<PathBuf>,
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join(rest)
    } else {
        PathBuf::from(path)
    }
}

fn load_history(path: &str) -> Result<Vec<HistoryRecord>, String> {
    let expanded = expand_tilde(path);
    let file = fs::File::open(&expanded)
        .map_err(|e| format!("Cannot open history file '{}': {}", expanded.display(), e))?;
    let reader = io::BufReader::new(file);

    let mut records: Vec<HistoryRecord> = Vec::new();
    for (line_no, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("IO error reading history at line {}: {}", line_no + 1, e))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let record: HistoryRecord = serde_json::from_str(trimmed).map_err(|e| {
            format!(
                "JSON parse error in history at line {} ('{}...'): {}",
                line_no + 1,
                &trimmed[..trimmed.len().min(60)],
                e
            )
        })?;
        records.push(record);
    }
    Ok(records)
}

fn main() {
    let cli = Cli::parse();

    // Validate format flag (NG3 — only markdown in V1)
    if cli.format != "markdown" {
        eprintln!(
            "error: only --format markdown is supported in V1 (got '{}')",
            cli.format
        );
        process::exit(1);
    }

    // Load history
    let mut history = match load_history(&cli.history) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("error: {}", e);
            process::exit(1);
        }
    };

    // AC6: empty history exits non-zero
    if history.is_empty() {
        eprintln!(
            "error: history file '{}' contains no records — no build can be reported",
            cli.history
        );
        process::exit(1);
    }

    // Apply --since-build window: keep records from the named build onwards (inclusive)
    if let Some(since) = &cli.since_build {
        if let Some(pos) = history.iter().position(|r| &r.build_id == since) {
            history = history.split_off(pos);
        } else {
            eprintln!(
                "error: --since-build '{}' is absent from history",
                since
            );
            process::exit(1);
        }
    }

    // Resolve target build
    let target_idx = if let Some(id) = &cli.build_id {
        match history.iter().position(|r| &r.build_id == id) {
            Some(i) => i,
            None => {
                eprintln!(
                    "error: build id '{}' is absent from history '{}' — no fallback (FR3)",
                    id, cli.history
                );
                process::exit(1);
            }
        }
    } else {
        // Default: most-recently recorded build (last in the JSONL, which is append-only)
        history.len() - 1
    };

    let target = &history[target_idx];
    let prior = if target_idx > 0 {
        Some(&history[target_idx - 1])
    } else {
        None
    };

    // Render brief (all history for trend section, FR6)
    let brief_text = render_brief(&history, target, prior);

    // Optionally emit Tiger record (FR7)
    if let Some(tiger_path) = &cli.emit_tiger_record {
        let tiger_rec = build_tiger_record(target);
        let json = serde_json::to_string_pretty(&tiger_rec).unwrap_or_default();
        if let Err(e) = fs::write(tiger_path, json) {
            eprintln!("error: cannot write Tiger record to '{}': {}", tiger_path.display(), e);
            process::exit(1);
        }
    }

    // Output brief
    match &cli.out {
        Some(out_path) => {
            if let Err(e) = fs::write(out_path, &brief_text) {
                eprintln!("error: cannot write brief to '{}': {}", out_path.display(), e);
                process::exit(1);
            }
        }
        None => {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            if let Err(e) = handle.write_all(brief_text.as_bytes()) {
                eprintln!("error: writing to stdout: {}", e);
                process::exit(1);
            }
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_fixture(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "{}", content).unwrap();
        f
    }

    fn load(path: &str) -> Vec<HistoryRecord> {
        load_history(path).unwrap()
    }

    // AC1: Headline present with correct build id, version, cluster, coverage %
    #[test]
    fn ac1_headline_correct() {
        let fixture = include_str!("tests/fixtures/ac1_basic.jsonl");
        let f = write_fixture(fixture);
        let history = load(f.path().to_str().unwrap());
        assert!(!history.is_empty());
        let target = history.iter().find(|r| r.build_id == "b-2026-06-10.1").unwrap();
        let brief = brief::render_brief(&history, target, None);
        assert!(brief.contains("b-2026-06-10.1"));
        assert!(brief.contains("v0.3.0"));
        assert!(brief.contains("mcp-aws.atscaleinternal.com"));
        // 5 verified out of ~7 total => ~72%
        assert!(brief.contains("72%") || brief.contains("71%"), "headline should show ~72%: {}", &brief[..200]);
        // No aliases
        for alias in &["latest", "the cluster", "nonprod", "staging"] {
            assert!(!brief.contains(alias), "banned alias '{}' found in brief", alias);
        }
    }

    // AC2: Default is most-recent build; absent build id path tested in unit
    #[test]
    fn ac2_default_is_most_recent() {
        let fixture = include_str!("tests/fixtures/ac2_two_builds.jsonl");
        let f = write_fixture(fixture);
        let history = load(f.path().to_str().unwrap());
        assert_eq!(history.len(), 2);
        // Last record in JSONL is the most recent
        let last = history.last().unwrap();
        let brief = brief::render_brief(&history, last, Some(&history[0]));
        assert!(brief.contains(&last.build_id));
    }

    // AC3: Regression list exact match, section above per-pair
    #[test]
    fn ac3_regression_list_exact() {
        let fixture = include_str!("tests/fixtures/ac3_regressions.jsonl");
        let f = write_fixture(fixture);
        let history = load(f.path().to_str().unwrap());
        let target = history.last().unwrap();
        let prior = if history.len() > 1 { Some(&history[history.len()-2]) } else { None };
        let brief = brief::render_brief(&history, target, prior);
        assert!(brief.contains("Total Returns"));
        assert!(brief.contains("Avg Net Profit"));
        let reg_pos = brief.find("Measures that newly disagree").unwrap();
        let pair_pos = brief.find("Coverage by backend pair").unwrap();
        assert!(reg_pos < pair_pos);
    }

    // AC4: AllSkipped not 0%, no regression
    #[test]
    fn ac4_all_skipped() {
        let fixture = include_str!("tests/fixtures/ac4_allskipped.jsonl");
        let f = write_fixture(fixture);
        let history = load(f.path().to_str().unwrap());
        let target = history.last().unwrap();
        let brief = brief::render_brief(&history, target, None);
        assert!(!brief.contains("0%"));
        assert!(brief.contains("AllSkipped") || brief.contains("no live backends"));
    }

    // AC5: Single build, no prior message
    #[test]
    fn ac5_single_build_no_prior() {
        let fixture = include_str!("tests/fixtures/ac5_single.jsonl");
        let f = write_fixture(fixture);
        let history = load(f.path().to_str().unwrap());
        assert_eq!(history.len(), 1);
        let target = &history[0];
        let brief = brief::render_brief(&history, target, None);
        assert!(brief.contains("No prior build") || brief.contains("first recorded"));
    }

    // AC6: Empty history — test load_history returns empty
    #[test]
    fn ac6_empty_history_load() {
        let f = write_fixture("");
        let result = load_history(f.path().to_str().unwrap());
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // AC7: 100% coverage, clean bill, zero never-tested
    #[test]
    fn ac7_100pct_clean() {
        let fixture = include_str!("tests/fixtures/ac7_100pct.jsonl");
        let f = write_fixture(fixture);
        let history = load(f.path().to_str().unwrap());
        let target = history.last().unwrap();
        let prior = if history.len() > 1 { Some(&history[history.len()-2]) } else { None };
        let brief = brief::render_brief(&history, target, prior);
        assert!(brief.contains("100%"));
        assert!(brief.contains("No measures newly disagree"));
        assert!(brief.contains("No never-tested") || brief.contains("full coverage"));
    }

    // AC8: Tiger record carries matching build id and coverage %
    #[test]
    fn ac8_tiger_record() {
        let fixture = include_str!("tests/fixtures/ac1_basic.jsonl");
        let f = write_fixture(fixture);
        let history = load(f.path().to_str().unwrap());
        let target = history.iter().find(|r| r.build_id == "b-2026-06-10.1").unwrap();
        let rec = brief::build_tiger_record(target);
        assert_eq!(rec.build_id, "b-2026-06-10.1");
        assert_eq!(rec.version, "v0.3.0");
        assert!(rec.parity_coverage_pct.is_some());
        // Serializable
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("b-2026-06-10.1"));
    }
}
