//! Integration tests for the `mqo-dax` binary.
//!
//! These tests invoke the binary via `std::process::Command` and assert on
//! stdout, stderr, and exit codes. They complement the library tests in
//! `tests/acceptance.rs` by exercising the binary dispatch path (AC7 of the
//! PRD / intent-card).
//!
//! # Building
//!
//! The binary must be compiled before these tests run. `cargo test` compiles
//! `[[bin]]` targets automatically, so `cargo test --test integration_cli` works
//! without a separate `cargo build` step.

use std::path::PathBuf;
use std::process::Command;

/// Absolute path to the compiled `mqo-dax` test binary.
fn mqo_dax_bin() -> PathBuf {
    // cargo sets CARGO_BIN_EXE_<name> for each [[bin]] in the workspace.
    // Use the env var if present (standard for `cargo test`).
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_mqo-dax") {
        return PathBuf::from(p);
    }
    // Fallback: look in target/debug or target/release relative to the
    // workspace root (CARGO_MANIFEST_DIR).
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR must be set by cargo");
    let root = PathBuf::from(manifest_dir);
    // Prefer debug for test speed; release is used by cargo test --release.
    let debug_bin = root.join("target/debug/mqo-dax");
    if debug_bin.exists() {
        return debug_bin;
    }
    root.join("target/release/mqo-dax")
}

/// Absolute path to the test fixtures directory.
fn fixtures_dir() -> PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR must be set by cargo");
    PathBuf::from(manifest_dir).join("tests/fixtures")
}

/// AC7-a: `mqo-dax --bound measure_only.json` writes valid DAX to stdout and exits 0.
#[test]
fn cli_measure_only_stdout() {
    let bin = mqo_dax_bin();
    let fixture = fixtures_dir().join("measure_only.json");

    let output = Command::new(&bin)
        .arg("--bound")
        .arg(&fixture)
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", bin.display()));

    assert!(
        output.status.success(),
        "expected exit 0, got {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EVALUATE"),
        "stdout must contain EVALUATE:\n{stdout}"
    );
    assert!(
        stdout.contains("[Revenue]"),
        "stdout must contain [Revenue]:\n{stdout}"
    );
}

/// AC7-b: `mqo-dax --bound /nonexistent/path.json` exits 2 (I/O error).
#[test]
fn cli_missing_file_exits_2() {
    let bin = mqo_dax_bin();

    let output = Command::new(&bin)
        .arg("--bound")
        .arg("/nonexistent/path/that/does/not/exist.json")
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", bin.display()));

    assert_eq!(
        output.status.code(),
        Some(2),
        "missing file must exit 2; got {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// AC7-c: `mqo-dax --bound empty_measures.json` exits 1 (compile error: EmptyMeasures).
#[test]
fn cli_empty_measures_exits_1() {
    let bin = mqo_dax_bin();
    let fixture = fixtures_dir().join("empty_measures.json");

    let output = Command::new(&bin)
        .arg("--bound")
        .arg(&fixture)
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", bin.display()));

    assert_eq!(
        output.status.code(),
        Some(1),
        "empty measures must exit 1; got {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("compile error") || stderr.contains("must have at least one measure"),
        "stderr must mention compile error: {stderr}"
    );
}

/// AC7-d: running with no arguments exits non-zero (clap usage error).
#[test]
fn cli_no_args_exits_nonzero() {
    let bin = mqo_dax_bin();

    let output = Command::new(&bin)
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", bin.display()));

    assert!(
        !output.status.success(),
        "no-args invocation must exit non-zero; got {:?}",
        output.status.code()
    );
}
