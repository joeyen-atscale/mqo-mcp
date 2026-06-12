//! AC4: Mode absent from baseline → reported but not gated; present modes still gated.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::doc_markdown)]

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

/// Trajectories: path_incompatible passes, wrong_date_role also passes.
/// Baseline only gates path_incompatible.
fn write_trajectories_both_pass(path: &std::path::Path) {
    let mut f = std::fs::File::create(path).unwrap();
    writeln!(f, r#"{{"task_id":"fm1-001","mcp":"nonprod","rollout":0,"answer":"path_incompatible — cannot answer","error":null,"rows":[],"final_sql":""}}"#).unwrap();
    writeln!(f, r#"{{"task_id":"fm1-002","mcp":"nonprod","rollout":0,"answer":"not compatible","error":null,"rows":[],"final_sql":""}}"#).unwrap();
    writeln!(f, r#"{{"task_id":"fm2-001","mcp":"nonprod","rollout":0,"answer":"ok","error":null,"rows":[{{"n":1}}],"final_sql":"SELECT \"Store Sales\", \"Sold Date Year\" FROM t"}}"#).unwrap();
    writeln!(f, r#"{{"task_id":"fm2-002","mcp":"nonprod","rollout":0,"answer":"ok","error":null,"rows":[{{"n":1}}],"final_sql":"SELECT \"Web Sales\", \"Sold Date Quarter\" FROM t"}}"#).unwrap();
}

#[test]
fn ac4_absent_mode_ungated_present_mode_gated() {
    let tmp = tempfile::tempdir().unwrap();
    let traj = tmp.path().join("traj.jsonl");
    write_trajectories_both_pass(&traj);

    // baseline_partial only gates path_incompatible.
    // wrong_date_role is present in corpus but absent from baseline.
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
        Some(0),
        "exit 0 when all gated modes pass\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    // wrong_date_role should appear in output (reported)
    assert!(
        stdout.contains("wrong_date_role"),
        "ungated mode should appear in output:\n{stdout}"
    );
    // It should be marked as ungated or no-floor.
    assert!(
        stdout.contains("ungated") || stdout.contains("no"),
        "ungated mode should be labeled:\n{stdout}"
    );
}

#[test]
fn ac4_absent_mode_does_not_cause_exit_one_when_failing() {
    // Even if wrong_date_role scores 0%, it should NOT cause exit 1 since it's ungated.
    let tmp = tempfile::tempdir().unwrap();
    let traj = tmp.path().join("traj.jsonl");
    let mut f = std::fs::File::create(&traj).unwrap();
    // path_incompatible passes
    writeln!(f, r#"{{"task_id":"fm1-001","mcp":"nonprod","rollout":0,"answer":"path_incompatible","error":null,"rows":[],"final_sql":""}}"#).unwrap();
    writeln!(f, r#"{{"task_id":"fm1-002","mcp":"nonprod","rollout":0,"answer":"path_incompatible","error":null,"rows":[],"final_sql":""}}"#).unwrap();
    // wrong_date_role fails (no correct measure/dim)
    writeln!(f, r#"{{"task_id":"fm2-001","mcp":"nonprod","rollout":0,"answer":"bad","error":null,"rows":[{{"n":1}}],"final_sql":"SELECT \"Wrong Measure\" FROM t"}}"#).unwrap();
    writeln!(f, r#"{{"task_id":"fm2-002","mcp":"nonprod","rollout":0,"answer":"bad","error":null,"rows":[{{"n":1}}],"final_sql":"SELECT \"Wrong Measure\" FROM t"}}"#).unwrap();
    drop(f);

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
        Some(0),
        "ungated failing mode must not cause exit 1\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
