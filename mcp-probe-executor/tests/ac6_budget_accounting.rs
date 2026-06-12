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
fn ac6_queries_run_equals_executed_not_proposed() {
    // With max_queries=1, only 1 hypothesis executes; budget.queries_run must be 1 not 2
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

    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("not valid JSON: {}\nstdout: {}", e, stdout));

    let queries_run = v["budget"]["queries_run"].as_u64().expect("queries_run is u64");
    assert_eq!(
        queries_run, 1,
        "queries_run should be 1 (only 1 executed before Halt), got {}",
        queries_run
    );

    // Verify 2 hypotheses were proposed but only 1 executed
    let resolved = v["resolved"].as_array().expect("resolved is array");
    assert_eq!(resolved.len(), 2, "should still have 2 resolved entries (one skipped_budget)");

    let executed_count = resolved
        .iter()
        .filter(|r| r["verdict"] != "skipped_budget")
        .count();
    assert_eq!(executed_count, 1, "only 1 hypothesis should have executed");
}

#[test]
fn ac6_queries_run_equals_all_when_no_halt() {
    // With unlimited budget, all 2 hypotheses execute; budget.queries_run must be 2
    let f = fixtures();
    let out = Command::new(binary())
        .args([
            "--hypotheses", f.join("hypset_two.json").to_str().unwrap(),
            "--budget",     f.join("budget_unlimited.json").to_str().unwrap(),
            "--summaries",  f.join("summaries").to_str().unwrap(),
            "--baseline",   f.join("summaries/baseline").to_str().unwrap(),
            "--now-ms",     "1000000",
            "--format",     "json",
        ])
        .output()
        .expect("failed to run binary");

    assert!(out.status.success());

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("not valid JSON: {}\nstdout: {}", e, stdout));

    let queries_run = v["budget"]["queries_run"].as_u64().expect("queries_run is u64");
    assert_eq!(queries_run, 2, "queries_run should be 2 for 2 executed hypotheses, got {}", queries_run);
}
