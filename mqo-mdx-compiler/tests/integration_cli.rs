//! Integration tests for the `mqo-mdx` binary.
//!
//! These tests invoke the binary via `std::process::Command` and assert on
//! stdout and exit codes — covering dispatch arms that `cargo test --release`
//! (lib-only) cannot reach.
//!
//! Per SKILL.md §"Binary integration test gap": for `--target lib` projects
//! that ship a `[[bin]]`, add `tests/integration_cli.rs` to catch missed
//! mutants in binary dispatch arms.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

/// Return the path to the compiled `mqo-mdx` binary under `target/`.
fn mqo_mdx_bin() -> PathBuf {
    // `cargo test` compiles the binary alongside tests; CARGO_BIN_EXE_mqo_mdx
    // is set by cargo's test harness to the exact path.
    // Fallback for manual runs: look in target/debug then target/release.
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_mqo-mdx") {
        return PathBuf::from(p);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for profile in &["debug", "release"] {
        let p = manifest.join("target").join(profile).join("mqo-mdx");
        if p.exists() {
            return p;
        }
    }
    panic!("mqo-mdx binary not found; run `cargo build` first");
}

/// Write `content` to a temp file and return its path.
/// The file lives for the duration of the test because we return the tempdir guard.
fn write_temp_json(content: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tmpdir");
    let path = dir.path().join("bound.json");
    let mut f = std::fs::File::create(&path).expect("create tmpfile");
    f.write_all(content.as_bytes()).expect("write tmpfile");
    (dir, path)
}

// Minimal BoundMqo JSON that compiles to a valid MDX SELECT (no dims = no row axis).
const SIMPLE_BOUND_JSON: &str = r#"{
  "mqo": {
    "model": "sales",
    "measures": [{"unique_name": "sales.revenue"}],
    "dimensions": [],
    "filters": [],
    "time_intelligence": [],
    "order": null,
    "limit": null,
    "non_empty": true
  },
  "measures": [
    {
      "unique_name": "sales.revenue",
      "is_calc": false,
      "semi_additive": false,
      "trigger_hierarchies": [],
      "mdx_dependency_hierarchies": []
    }
  ],
  "dimensions": [],
  "calc_group_members": []
}"#;

// BoundMqo with a semi-additive measure that lacks a trigger level → must exit 1.
const SEMI_ADDITIVE_NO_TRIGGER_JSON: &str = r#"{
  "mqo": {
    "model": "finance",
    "measures": [{"unique_name": "finance.end_balance"}],
    "dimensions": [],
    "filters": [],
    "time_intelligence": [],
    "order": null,
    "limit": null,
    "non_empty": true
  },
  "measures": [
    {
      "unique_name": "finance.end_balance",
      "is_calc": false,
      "semi_additive": true,
      "trigger_hierarchies": [],
      "mdx_dependency_hierarchies": []
    }
  ],
  "dimensions": [],
  "calc_group_members": []
}"#;

/// AC7-a: golden compile — binary exits 0 and stdout contains expected MDX.
#[test]
fn acceptance_cli_golden_compile() {
    let (_dir, path) = write_temp_json(SIMPLE_BOUND_JSON);
    let out = Command::new(mqo_mdx_bin())
        .arg("--bound")
        .arg(&path)
        .output()
        .expect("failed to run mqo-mdx");

    assert_eq!(
        out.status.code(),
        Some(0),
        "expected exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("FROM [sales]"),
        "expected FROM [sales] in stdout: {stdout}"
    );
    assert!(
        stdout.contains("[Measures].[Revenue]"),
        "expected Revenue measure in stdout: {stdout}"
    );
    assert!(
        !stdout.contains("ON ROWS"),
        "no dims → no row axis expected: {stdout}"
    );
}

/// AC7-b: semi-additive without trigger level → binary exits 1 (non-zero).
#[test]
fn acceptance_cli_semi_additive_exit_nonzero() {
    let (_dir, path) = write_temp_json(SEMI_ADDITIVE_NO_TRIGGER_JSON);
    let out = Command::new(mqo_mdx_bin())
        .arg("--bound")
        .arg(&path)
        .output()
        .expect("failed to run mqo-mdx");

    assert_ne!(
        out.status.code(),
        Some(0),
        "expected non-zero exit for missing trigger; stdout: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("semi-additive") || stderr.contains("trigger"),
        "expected error message mentioning semi-additive/trigger in stderr: {stderr}"
    );
}

/// AC7-c: missing --bound flag → binary exits 2 (usage error via clap).
#[test]
fn acceptance_cli_missing_bound_flag_exits_nonzero() {
    let out = Command::new(mqo_mdx_bin())
        .output()
        .expect("failed to run mqo-mdx");
    assert_ne!(
        out.status.code(),
        Some(0),
        "expected non-zero exit when --bound is missing"
    );
}

/// AC7-d: non-existent --bound file path → binary exits 2.
#[test]
fn acceptance_cli_nonexistent_bound_file_exits_2() {
    let out = Command::new(mqo_mdx_bin())
        .arg("--bound")
        .arg("/tmp/no-such-file-mqo-mdx-test.json")
        .output()
        .expect("failed to run mqo-mdx");
    assert_eq!(
        out.status.code(),
        Some(2),
        "expected exit 2 for missing file; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
