//! AC6: Malformed input → exit 2 with stderr diagnostic, no partial report.
#![allow(clippy::unwrap_used, clippy::expect_used)]

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

#[test]
fn ac6_nonexistent_tasks_file_exit_two() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(binary())
        .args([
            "--tasks",
            "/nonexistent/path/corpus.json",
            "--records",
            path_str(&tmp.path().join("traj.jsonl")),
            "--baseline",
            path_str(&fixtures().join("baseline_partial.json")),
        ])
        .output()
        .expect("failed to run mqoguard-regress");

    assert_eq!(
        out.status.code(),
        Some(2),
        "exit 2 for missing tasks file\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.is_empty(), "stderr should have a diagnostic");
}

#[test]
fn ac6_malformed_corpus_json_exit_two() {
    let tmp = tempfile::tempdir().unwrap();
    let bad_corpus = tmp.path().join("bad_corpus.json");
    std::fs::write(&bad_corpus, b"{ this is not valid json").unwrap();

    let traj = tmp.path().join("traj.jsonl");
    std::fs::write(&traj, b"").unwrap();

    let out = Command::new(binary())
        .args([
            "--tasks",
            path_str(&bad_corpus),
            "--records",
            path_str(&traj),
            "--baseline",
            path_str(&fixtures().join("baseline_partial.json")),
        ])
        .output()
        .expect("failed to run mqoguard-regress");

    assert_eq!(out.status.code(), Some(2), "exit 2 for malformed corpus");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.is_empty(), "stderr should have a diagnostic");
}

#[test]
fn ac6_malformed_trajectories_jsonl_exit_two() {
    let tmp = tempfile::tempdir().unwrap();
    let bad_traj = tmp.path().join("bad.jsonl");
    // First line is valid, second is garbage.
    std::fs::write(&bad_traj, b"{\"task_id\":\"x\"}\nnot json at all\n").unwrap();

    let out = Command::new(binary())
        .args([
            "--tasks",
            path_str(&fixtures().join("corpus_small.json")),
            "--records",
            path_str(&bad_traj),
            "--baseline",
            path_str(&fixtures().join("baseline_partial.json")),
        ])
        .output()
        .expect("failed to run mqoguard-regress");

    assert_eq!(
        out.status.code(),
        Some(2),
        "exit 2 for malformed JSONL\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn ac6_malformed_baseline_exit_two() {
    let tmp = tempfile::tempdir().unwrap();
    let bad_baseline = tmp.path().join("bad_baseline.json");
    std::fs::write(&bad_baseline, b"[1,2,3]").unwrap(); // array, not object

    let traj = tmp.path().join("traj.jsonl");
    std::fs::write(&traj, b"").unwrap();

    let out = Command::new(binary())
        .args([
            "--tasks",
            path_str(&fixtures().join("corpus_small.json")),
            "--records",
            path_str(&traj),
            "--baseline",
            path_str(&bad_baseline),
        ])
        .output()
        .expect("failed to run mqoguard-regress");

    assert_eq!(
        out.status.code(),
        Some(2),
        "exit 2 for malformed baseline\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
