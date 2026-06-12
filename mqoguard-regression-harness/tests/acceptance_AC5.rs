//! AC5: `--format json` emits machine-readable report with per-mode scores, floors, pass/fail, exit decision.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::process::Command;

fn binary() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target/debug/mqoguard-regress");
    p
}

fn fixtures() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p
}

fn path_str(p: &std::path::Path) -> &str {
    p.to_str().expect("valid utf-8 path")
}

fn write_passing(path: &std::path::Path) {
    let mut f = std::fs::File::create(path).unwrap();
    writeln!(f, r#"{{"task_id":"fm1-001","mcp":"nonprod","rollout":0,"answer":"path_incompatible","error":null,"rows":[],"final_sql":""}}"#).unwrap();
    writeln!(f, r#"{{"task_id":"fm1-002","mcp":"nonprod","rollout":0,"answer":"path_incompatible","error":null,"rows":[],"final_sql":""}}"#).unwrap();
}

#[test]
fn ac5_json_format_well_formed() {
    let tmp = tempfile::tempdir().unwrap();
    let traj = tmp.path().join("traj.jsonl");
    write_passing(&traj);

    let out = Command::new(binary())
        .args([
            "--tasks",
            path_str(&fixtures().join("corpus_small.json")),
            "--records",
            path_str(&traj),
            "--baseline",
            path_str(&fixtures().join("baseline_partial.json")),
            "--format",
            "json",
        ])
        .output()
        .expect("failed to run mqoguard-regress");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let parse_result: Result<serde_json::Value, _> = serde_json::from_str(&stdout);
    assert!(parse_result.is_ok(), "JSON output not parseable\n---\n{stdout}");
    let v = parse_result.unwrap();

    // Must have top-level keys: modes, overall, tolerance.
    assert!(v.get("modes").is_some(), "missing 'modes' key:\n{stdout}");
    assert!(v.get("overall").is_some(), "missing 'overall' key:\n{stdout}");
    assert!(v.get("tolerance").is_some(), "missing 'tolerance' key:\n{stdout}");

    // overall must have any_below_floor and failing_modes.
    let overall = &v["overall"];
    assert!(overall.get("any_below_floor").is_some(), "missing overall.any_below_floor");
    assert!(overall.get("failing_modes").is_some(), "missing overall.failing_modes");

    // Each mode entry must have required fields.
    let modes = v["modes"].as_array().expect("modes must be array");
    assert!(!modes.is_empty(), "modes array empty");
    for mode in modes {
        assert!(mode.get("mode").is_some(), "mode missing 'mode' field");
        assert!(mode.get("path_mean").is_some(), "mode missing 'path_mean'");
        assert!(mode.get("pass_at_k").is_some(), "mode missing 'pass_at_k'");
        assert!(mode.get("is_gated").is_some(), "mode missing 'is_gated'");
        assert!(mode.get("path_mean_ok").is_some(), "mode missing 'path_mean_ok'");
    }
}

#[test]
fn ac5_json_format_exit_zero_when_passing() {
    let tmp = tempfile::tempdir().unwrap();
    let traj = tmp.path().join("traj.jsonl");
    write_passing(&traj);

    let out = Command::new(binary())
        .args([
            "--tasks",
            path_str(&fixtures().join("corpus_small.json")),
            "--records",
            path_str(&traj),
            "--baseline",
            path_str(&fixtures().join("baseline_partial.json")),
            "--format",
            "json",
        ])
        .output()
        .expect("failed to run mqoguard-regress");

    assert_eq!(out.status.code(), Some(0), "should exit 0 when passing");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["overall"]["any_below_floor"], serde_json::Value::Bool(false));
}
