//! CLI acceptance tests for `parity-tracker`.
//!
//! Each test maps to one or more ACs from PRD-mqo-parity-coverage-tracker.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use assert_cmd::Command;
use tempfile::TempDir;

fn bin() -> Command {
    Command::cargo_bin("parity-tracker").unwrap()
}

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

// ── AC-1: record stamps build-id and appends exactly one JSONL line ──────────

#[test]
fn ac1_record_stamps_build_id_appends_one_line() {
    let dir = TempDir::new().unwrap();
    let history = dir.path().join("h.jsonl");
    let report = fixtures().join("report_tss_equal.json");

    bin()
        .args([
            "record",
            "--input", report.to_str().unwrap(),
            "--build-id", "2026.06.10-rc1",
            "--history", history.to_str().unwrap(),
        ])
        .assert()
        .success();

    let content = fs::read_to_string(&history).unwrap();
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "exactly one JSONL line should be appended");

    let entry: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(entry["build_id"], "2026.06.10-rc1");
    assert_eq!(entry["all_skipped"], false);
}

#[test]
fn ac1_record_preserves_prior_lines() {
    let dir = TempDir::new().unwrap();
    let history = dir.path().join("h.jsonl");
    let report = fixtures().join("report_tss_equal.json");

    bin()
        .args(["record", "--input", report.to_str().unwrap(),
               "--build-id", "build-A", "--history", history.to_str().unwrap()])
        .assert().success();

    bin()
        .args(["record", "--input", report.to_str().unwrap(),
               "--build-id", "build-B", "--history", history.to_str().unwrap()])
        .assert().success();

    let lines: Vec<_> = fs::read_to_string(&history).unwrap()
        .lines().filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["build_id"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(lines, ["build-A", "build-B"]);
}

// ── AC-2: coverage reports correct status grid and % ─────────────────────────

#[test]
fn ac2_coverage_one_of_three_verified_json() {
    let dir = TempDir::new().unwrap();
    let history = dir.path().join("h.jsonl");
    let report = fixtures().join("report_tss_equal.json");
    let corpus = fixtures().join("corpus.json");

    bin()
        .args(["record", "--input", report.to_str().unwrap(),
               "--build-id", "build-B", "--history", history.to_str().unwrap()])
        .assert().success();

    let out = bin()
        .args(["coverage", "--build-id", "build-B",
               "--corpus", corpus.to_str().unwrap(),
               "--history", history.to_str().unwrap(),
               "--format", "json"])
        .assert().success()
        .get_output()
        .stdout.clone();

    let summary: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(summary["verified_count"], 1);
    assert_eq!(summary["never_tested_count"], 2);
    assert_eq!(summary["denominator"], 3);
    let pct = summary["coverage_pct"].as_f64().unwrap();
    assert!((pct - 33.333_f64).abs() < 0.01, "expected ~33.3% got {pct}");

    let cells = summary["cells"].as_array().unwrap();
    let tss = cells.iter().find(|c| c["measure"] == "Total Store Sales").unwrap();
    assert_eq!(tss["status"], "verified");
    let margin = cells.iter().find(|c| c["measure"] == "Margin %").unwrap();
    assert_eq!(margin["status"], "never_tested");
}

// ── AC-3: delta detects newly-broken, exits non-zero by default ───────────────

#[test]
fn ac3_delta_newly_broken_exits_nonzero() {
    let dir = TempDir::new().unwrap();
    let history = dir.path().join("h.jsonl");
    let corpus = fixtures().join("corpus.json");
    let equal_report = fixtures().join("report_tss_equal.json");
    let mismatch_report = fixtures().join("report_margin_mismatch.json");

    // N-1: Margin % = Equal
    let margin_equal = r#"{"mqo_path":"Margin.json","measure":"Margin %","pairs":[["dax","sql","Equal"]],"overall":"Agree"}"#;
    let margin_equal_path = dir.path().join("margin_equal.json");
    fs::write(&margin_equal_path, margin_equal).unwrap();

    bin()
        .args(["record", "--input", margin_equal_path.to_str().unwrap(),
               "--build-id", "build-N-1", "--history", history.to_str().unwrap()])
        .assert().success();

    // N: Margin % = Mismatch
    bin()
        .args(["record", "--input", mismatch_report.to_str().unwrap(),
               "--build-id", "build-N", "--history", history.to_str().unwrap()])
        .assert().success();

    // delta should exit 2 (newly-broken present)
    let out = bin()
        .args(["delta", "--build", "build-N", "--against", "build-N-1",
               "--corpus", corpus.to_str().unwrap(),
               "--history", history.to_str().unwrap(),
               "--format", "json"])
        .assert().code(2)
        .get_output()
        .stdout.clone();

    let delta: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let broken = delta["newly_broken"].as_array().unwrap();
    assert!(broken.iter().any(|m| m == "Margin %"), "Margin % should be in newly_broken");

    // with --no-fail, same input should exit 0
    bin()
        .args(["delta", "--build", "build-N", "--against", "build-N-1",
               "--corpus", corpus.to_str().unwrap(),
               "--history", history.to_str().unwrap(),
               "--no-fail"])
        .assert().success();
}

// ── AC-4: measure in corpus with no result → never-tested ────────────────────

#[test]
fn ac4_no_result_for_measure_is_never_tested() {
    let dir = TempDir::new().unwrap();
    let corpus = fixtures().join("corpus.json");

    // No history at all — missing history file path
    let out = bin()
        .args(["coverage", "--build-id", "build-X",
               "--corpus", corpus.to_str().unwrap(),
               "--history", dir.path().join("absent.jsonl").to_str().unwrap(),
               "--format", "json"])
        .assert().success()
        .get_output()
        .stdout.clone();

    let summary: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(summary["never_tested_count"], 3);
    assert_eq!(summary["verified_count"], 0);

    let cells = summary["cells"].as_array().unwrap();
    for cell in cells {
        assert_eq!(cell["status"], "never_tested");
    }
}

// ── AC-5: AllSkipped result recorded but does not change coverage ─────────────

#[test]
fn ac5_all_skipped_does_not_change_coverage() {
    let dir = TempDir::new().unwrap();
    let history = dir.path().join("h.jsonl");
    let corpus = fixtures().join("corpus.json");
    let equal_report = fixtures().join("report_tss_equal.json");
    let skipped_report = fixtures().join("report_all_skipped.json");

    // Record verified result for TSS
    bin()
        .args(["record", "--input", equal_report.to_str().unwrap(),
               "--build-id", "build-B", "--history", history.to_str().unwrap()])
        .assert().success();

    // Get baseline coverage
    let base_out = bin()
        .args(["coverage", "--build-id", "build-B",
               "--corpus", corpus.to_str().unwrap(),
               "--history", history.to_str().unwrap(),
               "--format", "json"])
        .assert().success()
        .get_output().stdout.clone();
    let base: serde_json::Value = serde_json::from_slice(&base_out).unwrap();

    // Now record AllSkipped at same build
    bin()
        .args(["record", "--input", skipped_report.to_str().unwrap(),
               "--build-id", "build-B", "--history", history.to_str().unwrap()])
        .assert().success();

    // Coverage should be unchanged
    let after_out = bin()
        .args(["coverage", "--build-id", "build-B",
               "--corpus", corpus.to_str().unwrap(),
               "--history", history.to_str().unwrap(),
               "--format", "json"])
        .assert().success()
        .get_output().stdout.clone();
    let after: serde_json::Value = serde_json::from_slice(&after_out).unwrap();

    assert_eq!(base["verified_count"], after["verified_count"]);
    assert_eq!(base["coverage_pct"], after["coverage_pct"]);
}

// ── AC-6: --format json and --format human both succeed ──────────────────────

#[test]
fn ac6_format_json_and_human_both_succeed() {
    let dir = TempDir::new().unwrap();
    let history = dir.path().join("h.jsonl");
    let corpus = fixtures().join("corpus.json");
    let report = fixtures().join("report_tss_equal.json");

    bin()
        .args(["record", "--input", report.to_str().unwrap(),
               "--build-id", "build-B", "--history", history.to_str().unwrap()])
        .assert().success();

    // coverage --format json
    let json_out = bin()
        .args(["coverage", "--build-id", "build-B",
               "--corpus", corpus.to_str().unwrap(),
               "--history", history.to_str().unwrap(),
               "--format", "json"])
        .assert().success()
        .get_output().stdout.clone();
    serde_json::from_slice::<serde_json::Value>(&json_out)
        .expect("--format json must produce valid JSON");

    // coverage --format human
    let human_out = bin()
        .args(["coverage", "--build-id", "build-B",
               "--corpus", corpus.to_str().unwrap(),
               "--history", history.to_str().unwrap(),
               "--format", "human"])
        .assert().success()
        .get_output().stdout.clone();
    let human_str = String::from_utf8(human_out).unwrap();
    assert!(human_str.contains('%'), "human coverage output must contain '%'");
}

// ── AC-7: corrupt trailing JSONL line is skipped, valid results returned ─────

#[test]
fn ac7_corrupt_trailing_line_skipped_valid_results_returned() {
    let dir = TempDir::new().unwrap();
    let history = dir.path().join("h.jsonl");
    let corpus = fixtures().join("corpus.json");
    let report = fixtures().join("report_tss_equal.json");

    // Write one valid entry
    bin()
        .args(["record", "--input", report.to_str().unwrap(),
               "--build-id", "build-B", "--history", history.to_str().unwrap()])
        .assert().success();

    // Append a corrupt (truncated) line
    let mut f = std::fs::OpenOptions::new().append(true).open(&history).unwrap();
    writeln!(f, r#"{{"corrupt": true, "incomplete_json""#).unwrap();

    // coverage must still succeed and return correct results
    let out = bin()
        .args(["coverage", "--build-id", "build-B",
               "--corpus", corpus.to_str().unwrap(),
               "--history", history.to_str().unwrap(),
               "--format", "json"])
        .assert().success()
        .get_output().stdout.clone();

    let summary: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(summary["verified_count"], 1, "valid entries must still count");
}

// ── AC-8: empty history / unknown build-id → all never-tested, exit 0 ────────

#[test]
fn ac8_empty_history_all_never_tested_exit_zero() {
    let dir = TempDir::new().unwrap();
    let corpus = fixtures().join("corpus.json");

    // Explicitly empty history file
    let history = dir.path().join("empty.jsonl");
    fs::write(&history, "").unwrap();

    let out = bin()
        .args(["coverage", "--build-id", "never-seen-build",
               "--corpus", corpus.to_str().unwrap(),
               "--history", history.to_str().unwrap(),
               "--format", "json"])
        .assert().success()
        .get_output().stdout.clone();

    let summary: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(summary["verified_count"], 0);
    assert_eq!(summary["coverage_pct"], 0.0);
    assert!(summary["never_tested_count"].as_u64().unwrap() > 0);
}
