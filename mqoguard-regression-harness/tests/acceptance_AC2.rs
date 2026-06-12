//! AC2: `path_incompatible` mode below its baseline floor → exit 1, report names failing mode.
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

/// All `path_incompatible` tasks fail (rows returned = fabrication).
fn write_failing_pi_trajectories(path: &std::path::Path) {
    let mut f = std::fs::File::create(path).unwrap();
    writeln!(f, r#"{{"task_id":"fm1-001","mcp":"nonprod","rollout":0,"answer":"here are the results","error":null,"rows":[{{"n":42}}],"final_sql":"SELECT 1"}}"#).unwrap();
    writeln!(f, r#"{{"task_id":"fm1-002","mcp":"nonprod","rollout":0,"answer":"found results","error":null,"rows":[{{"n":7}}],"final_sql":"SELECT 2"}}"#).unwrap();
    // wrong_date_role tasks pass fine.
    writeln!(f, r#"{{"task_id":"fm2-001","mcp":"nonprod","rollout":0,"answer":"ok","error":null,"rows":[{{"n":100}}],"final_sql":"SELECT \"Store Sales\", \"Sold Date Year\" FROM t"}}"#).unwrap();
    writeln!(f, r#"{{"task_id":"fm2-002","mcp":"nonprod","rollout":0,"answer":"ok","error":null,"rows":[{{"n":50}}],"final_sql":"SELECT \"Web Sales\", \"Sold Date Quarter\" FROM t"}}"#).unwrap();
}

#[test]
fn ac2_path_incompatible_below_floor_exit_one() {
    let tmp = tempfile::tempdir().unwrap();
    let traj = tmp.path().join("trajectories.jsonl");
    write_failing_pi_trajectories(&traj);

    // baseline_partial gates path_incompatible at floor 22.5%.
    // 0% < 22.5% → should exit 1.
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

    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1 on regression\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("path_incompatible"),
        "report must name the failing mode:\n{stdout}"
    );
    assert!(
        stdout.contains("REGRESSION") || stdout.contains("FAIL"),
        "report must indicate regression:\n{stdout}"
    );
}
