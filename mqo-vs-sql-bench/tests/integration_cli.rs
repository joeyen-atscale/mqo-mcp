//! CLI binary integration tests for mqo-bench.
//!
//! Invokes the compiled binary via std::process::Command and asserts on
//! exit codes and stdout shapes. This covers the binary dispatch arms that
//! cargo test --lib cannot reach.
//!
//! Requires `cargo build --release` to have run (uses CARGO_BIN_EXE_mqo-bench).

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn bin() -> PathBuf {
    // Use the env var set by cargo test for the binary path.
    PathBuf::from(env!("CARGO_BIN_EXE_mqo-bench"))
}

fn stub_grader() -> String {
    fixture("fixtures/stub_grader.sh")
        .to_string_lossy()
        .into_owned()
}

// ── CLI smoke tests ────────────────────────────────────────────────────────

/// Binary exists and exits 0 with --help.
#[test]
fn cli_help_exits_zero() {
    let out = Command::new(bin()).arg("--help").output().expect("binary must run");
    assert!(out.status.success(), "--help must exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("mqo-bench") || stdout.contains("MQO"), "--help must mention mqo-bench");
}

/// Missing --tasks flag exits non-zero.
#[test]
fn cli_no_tasks_flag_exits_nonzero() {
    let out = Command::new(bin()).output().expect("binary must run");
    assert!(
        !out.status.success(),
        "missing --tasks must exit non-zero"
    );
}

/// Non-existent tasks file exits 1.
#[test]
fn cli_nonexistent_tasks_exits_one() {
    let out = Command::new(bin())
        .args(["--tasks", "/definitely/does/not/exist.json"])
        .output()
        .expect("binary must run");
    assert_eq!(
        out.status.code(),
        Some(1),
        "non-existent tasks file must exit with code 1"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("error"), "stderr must mention error");
}

/// Full end-to-end run via binary: tasks + fixtures + stub grader → JSON + Markdown to stdout.
#[test]
fn cli_full_run_with_fixtures_produces_json_stdout() {
    let tasks = fixture("fixtures/tasks.json");
    let fix_a = fixture("fixtures/arm_a_outputs.json");
    let fix_b = fixture("fixtures/arm_b_outputs.json");

    let out = Command::new(bin())
        .args([
            "--tasks",
            tasks.to_str().unwrap(),
            "--grader",
            &stub_grader(),
            "--fixture-a",
            fix_a.to_str().unwrap(),
            "--fixture-b",
            fix_b.to_str().unwrap(),
        ])
        .output()
        .expect("binary must run");

    assert!(
        out.status.success(),
        "full fixture run must succeed; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    // JSON output must contain aggregate and questions keys.
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim().lines().next().unwrap_or("")).unwrap_or_else(|_| {
            // Fallback: try parsing the whole stdout as JSON (when no --output-json,
            // main prints JSON then Markdown; grab up to the first markdown heading).
            let json_part: String = stdout
                .lines()
                .take_while(|l| !l.starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n");
            serde_json::from_str(json_part.trim()).unwrap_or(serde_json::Value::Null)
        });

    assert!(v.get("aggregate").is_some(), "stdout JSON must have 'aggregate' key");
    assert!(v.get("questions").is_some(), "stdout JSON must have 'questions' key");

    let n = v["questions"].as_array().map(|a| a.len()).unwrap_or(0);
    assert_eq!(n, 3, "3 fixture tasks → 3 question results");

    // Markdown section must also be present in stdout.
    assert!(
        stdout.contains("# MQO vs SQL Benchmark Report"),
        "stdout must contain Markdown report heading"
    );
}

/// --output-json writes file; stdout gets Markdown.
#[test]
fn cli_output_json_writes_file_and_stdout_gets_markdown() {
    use std::fs;

    let dir = tempdir();
    let json_out = dir.join("report.json");

    let tasks = fixture("fixtures/tasks.json");
    let fix_a = fixture("fixtures/arm_a_outputs.json");
    let fix_b = fixture("fixtures/arm_b_outputs.json");

    let out = Command::new(bin())
        .args([
            "--tasks",
            tasks.to_str().unwrap(),
            "--grader",
            &stub_grader(),
            "--fixture-a",
            fix_a.to_str().unwrap(),
            "--fixture-b",
            fix_b.to_str().unwrap(),
            "--output-json",
            json_out.to_str().unwrap(),
        ])
        .output()
        .expect("binary must run");

    assert!(
        out.status.success(),
        "run with --output-json must succeed; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // JSON file must exist and be parseable.
    let json_str = fs::read_to_string(&json_out).expect("--output-json file must be written");
    let v: serde_json::Value =
        serde_json::from_str(&json_str).expect("--output-json file must be valid JSON");
    assert!(v.get("aggregate").is_some());
    assert!(v.get("questions").is_some());

    // stdout should contain Markdown (since --output-md was not set).
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("# MQO vs SQL Benchmark Report"),
        "stdout must contain Markdown when --output-json is used without --output-md"
    );

    cleanup_dir(dir);
}

// ── helpers ────────────────────────────────────────────────────────────────

fn tempdir() -> PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("mqo_bench_cli_test_{ts}"));
    std::fs::create_dir_all(&dir).expect("temp dir must be creatable");
    dir
}

fn cleanup_dir(dir: PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}
