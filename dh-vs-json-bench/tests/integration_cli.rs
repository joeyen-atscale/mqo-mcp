//! Integration tests that invoke the `dh-bench` binary directly via
//! `std::process::Command`.
//!
//! These catch dispatch/exit-code bugs in `main.rs` that library-only tests
//! cannot reach (per autobuilder binary integration test gap, 2026-06-08).
//!
//! All tests run fully offline using bundled fixtures and the stub grader.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve a path relative to the crate root.
fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Path to the compiled `dh-bench` binary (debug or release).
fn dh_bench_bin() -> PathBuf {
    // `CARGO_BIN_EXE_dh-bench` is set by cargo when running integration tests.
    // Fall back to `target/debug/dh-bench` for direct invocations.
    if let Some(p) = option_env!("CARGO_BIN_EXE_dh-bench") {
        PathBuf::from(p)
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug")
            .join("dh-bench")
    }
}

fn stub_grader() -> String {
    fixture("fixtures/stub_grader.sh")
        .to_string_lossy()
        .into_owned()
}

fn tasks_path() -> String {
    fixture("fixtures/tasks.json")
        .to_string_lossy()
        .into_owned()
}

fn fixture_a_path() -> String {
    fixture("fixtures/arm_a_outputs.json")
        .to_string_lossy()
        .into_owned()
}

fn fixture_b_path() -> String {
    fixture("fixtures/arm_b_outputs.json")
        .to_string_lossy()
        .into_owned()
}

/// CLI: missing --tasks flag → non-zero exit.
#[test]
fn cli_missing_tasks_exits_nonzero() {
    let status = Command::new(dh_bench_bin())
        .output()
        .expect("dh-bench binary must exist");
    assert!(
        !status.status.success(),
        "dh-bench with no args must exit non-zero"
    );
}

/// CLI: non-existent tasks file → exit 1 with error message.
#[test]
fn cli_nonexistent_tasks_file_exits_1() {
    let output = Command::new(dh_bench_bin())
        .args(["--tasks", "/nonexistent/tasks.json"])
        .output()
        .expect("dh-bench binary must exist");
    assert_eq!(
        output.status.code(),
        Some(1),
        "non-existent tasks file must exit with code 1"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("error"),
        "stderr must mention 'error' on missing tasks file"
    );
}

/// CLI: fixture run succeeds (exit 0) and JSON output contains 'aggregate'.
#[test]
fn cli_fixture_run_exits_0_and_emits_json() {
    let output = Command::new(dh_bench_bin())
        .args([
            "--tasks",
            &tasks_path(),
            "--grader",
            &stub_grader(),
            "--fixture-a",
            &fixture_a_path(),
            "--fixture-b",
            &fixture_b_path(),
        ])
        .output()
        .expect("dh-bench binary must exist");

    assert!(
        output.status.success(),
        "fixture run must exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("aggregate"),
        "stdout must contain 'aggregate' key in JSON output"
    );
    assert!(
        stdout.contains("questions"),
        "stdout must contain 'questions' key in JSON output"
    );
}

/// CLI: --output-json writes a file; --output-md writes a file.
#[test]
fn cli_output_flags_write_files() {
    let tmp = std::env::temp_dir();
    let json_out = tmp.join("dh_bench_test_out.json");
    let md_out = tmp.join("dh_bench_test_out.md");

    // Clean up before test.
    let _ = std::fs::remove_file(&json_out);
    let _ = std::fs::remove_file(&md_out);

    let output = Command::new(dh_bench_bin())
        .args([
            "--tasks",
            &tasks_path(),
            "--grader",
            &stub_grader(),
            "--fixture-a",
            &fixture_a_path(),
            "--fixture-b",
            &fixture_b_path(),
            "--output-json",
            json_out.to_str().unwrap(),
            "--output-md",
            md_out.to_str().unwrap(),
        ])
        .output()
        .expect("dh-bench binary must exist");

    assert!(
        output.status.success(),
        "fixture run with output flags must exit 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    // JSON file must exist and contain 'aggregate'.
    let json_content = std::fs::read_to_string(&json_out)
        .expect("--output-json must create the file");
    assert!(
        json_content.contains("aggregate"),
        "JSON output file must contain 'aggregate'"
    );

    // Markdown file must exist and contain the report heading.
    let md_content = std::fs::read_to_string(&md_out)
        .expect("--output-md must create the file");
    assert!(
        md_content.contains("# Handle vs Raw-JSON Benchmark Report"),
        "Markdown output file must contain main heading"
    );

    // Cleanup.
    let _ = std::fs::remove_file(&json_out);
    let _ = std::fs::remove_file(&md_out);
}

/// CLI: empty tasks file → exit 1 with 'empty' in error message.
#[test]
fn cli_empty_tasks_file_exits_1() {
    // Write a temp file with an empty JSON array.
    let tmp = std::env::temp_dir().join("dh_bench_empty_tasks.json");
    std::fs::write(&tmp, "[]").expect("must write temp tasks file");

    let output = Command::new(dh_bench_bin())
        .args(["--tasks", tmp.to_str().unwrap()])
        .output()
        .expect("dh-bench binary must exist");

    assert_eq!(
        output.status.code(),
        Some(1),
        "empty tasks file must exit with code 1"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("empty"),
        "stderr must mention 'empty' when tasks file is empty"
    );

    let _ = std::fs::remove_file(&tmp);
}
