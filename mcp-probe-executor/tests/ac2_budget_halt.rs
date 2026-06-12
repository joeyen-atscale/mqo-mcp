use std::process::Command;

fn binary() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("target/debug/mcp-probe-executor");
    p
}

fn fixtures() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p
}

#[test]
fn ac2_budget_halt() {
    // max_queries=1: first hypothesis executes, second gets Halt → skipped_budget
    let f = fixtures();
    let out = Command::new(binary())
        .args([
            "--hypotheses", f.join("hypset_two.json").to_str().unwrap(),
            "--budget",     f.join("budget_one.json").to_str().unwrap(),
            "--summaries",  f.join("summaries").to_str().unwrap(),
            "--baseline",   f.join("summaries/baseline").to_str().unwrap(),
            "--now-ms",     "1000000",
            "--format",     "json",
        ])
        .output()
        .expect("failed to run binary");

    assert!(
        out.status.success(),
        "binary exited with error: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("not valid JSON: {}\nstdout: {}", e, stdout));

    // halted must be true
    assert_eq!(v["halted"], true, "expected halted=true");

    // budget.verdict_at_stop must be "Halt"
    assert_eq!(v["budget"]["verdict_at_stop"], "Halt");

    let resolved = v["resolved"].as_array().expect("resolved is array");
    assert_eq!(resolved.len(), 2);

    // First hypothesis should be executed (confirmed/refuted, not skipped)
    let r1 = &resolved[0];
    assert_ne!(r1["verdict"], "skipped_budget", "rank 1 should have executed");

    // Second hypothesis should be skipped_budget
    let r2 = &resolved[1];
    assert_eq!(r2["verdict"], "skipped_budget", "rank 2 should be skipped_budget");
}
