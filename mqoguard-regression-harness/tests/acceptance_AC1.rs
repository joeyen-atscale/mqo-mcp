//! AC1: All modes meet their floor → exit 0.
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

/// Write a JSONL file with all modes passing their floors.
fn write_passing_trajectories(path: &std::path::Path) {
    let mut f = std::fs::File::create(path).unwrap();
    // path_incompatible tasks: both pass via rejection keyword
    writeln!(f, r#"{{"task_id":"fm1-001","mcp":"nonprod","rollout":0,"answer":"this query is path_incompatible and cannot be answered","error":null,"rows":[],"final_sql":""}}"#).unwrap();
    writeln!(f, r#"{{"task_id":"fm1-002","mcp":"nonprod","rollout":0,"answer":"incompatible path, rejecting","error":null,"rows":[],"final_sql":""}}"#).unwrap();
    // wrong_date_role tasks: pass with correct measure + dim
    writeln!(f, r#"{{"task_id":"fm2-001","mcp":"nonprod","rollout":0,"answer":"ok","error":null,"rows":[{{"n":100}}],"final_sql":"SELECT \"Store Sales\", \"Sold Date Year\" FROM t"}}"#).unwrap();
    writeln!(f, r#"{{"task_id":"fm2-002","mcp":"nonprod","rollout":0,"answer":"ok","error":null,"rows":[{{"n":50}}],"final_sql":"SELECT \"Web Sales\", \"Sold Date Quarter\" FROM t"}}"#).unwrap();
}

#[test]
fn ac1_all_modes_pass_exit_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let traj = tmp.path().join("trajectories.jsonl");
    write_passing_trajectories(&traj);

    // baseline_partial only gates path_incompatible (floor 22.5%).
    // Both fm1 tasks pass → path_mean = 100% → well above floor.
    let out = Command::new(binary())
        .args([
            "--tasks",
            path_str(&fixtures().join("corpus_small.json")),
            "--records",
            path_str(&traj),
            "--baseline",
            path_str(&fixtures().join("baseline_partial.json")),
        ])
        .output()
        .expect("failed to run mqoguard-regress");

    assert_eq!(out.status.code(), Some(0),
        "expected exit 0 when all modes pass\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("OK:"), "expected OK line in output:\n{stdout}");
}
