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
fn ac3_checkin_sets_flag_and_continues() {
    // budget_checkin.json: max_queries=2, checkin_fraction=0.4
    // After 1st query: queries_run=1, fraction=1/2=0.5 >= 0.4 → CheckIn on 2nd hypothesis check
    // Both hypotheses should still resolve (loop does NOT stop on CheckIn)
    let f = fixtures();
    let out = Command::new(binary())
        .args([
            "--hypotheses", f.join("hypset_two.json").to_str().unwrap(),
            "--budget",     f.join("budget_checkin.json").to_str().unwrap(),
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

    // checkin_pending must be true
    assert_eq!(v["checkin_pending"], true, "expected checkin_pending=true");

    // halted must NOT be true
    assert_eq!(v["halted"], false, "expected halted=false (CheckIn does not halt)");

    // Both hypotheses should have resolved (not skipped_budget)
    let resolved = v["resolved"].as_array().expect("resolved is array");
    assert_eq!(resolved.len(), 2);
    for r in resolved {
        assert_ne!(
            r["verdict"], "skipped_budget",
            "CheckIn should not skip hypotheses, got verdict: {}",
            r["verdict"]
        );
    }
}
