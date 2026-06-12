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
fn ac1_confirm_and_refute() {
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

    assert!(
        out.status.success(),
        "binary exited with error: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("output is not valid JSON: {}\nstdout: {}", e, stdout));

    let resolved = v["resolved"].as_array().expect("resolved is array");
    assert_eq!(resolved.len(), 2);

    // rank=1 predicted down, delta=-0.075 → confirmed
    let r1 = &resolved[0];
    assert_eq!(r1["rank"], 1);
    assert_eq!(r1["verdict"], "confirmed", "rank 1 should be confirmed");
    let delta1 = r1["observed_delta_fraction"].as_f64().expect("delta is f64");
    assert!(
        (delta1 - (-0.075)).abs() < 1e-9,
        "expected delta -0.075 got {}",
        delta1
    );

    // rank=2 predicted down, delta=+0.06 → refuted
    let r2 = &resolved[1];
    assert_eq!(r2["rank"], 2);
    assert_eq!(r2["verdict"], "refuted", "rank 2 should be refuted");

    // evidence_type must be "structural"
    assert_eq!(v["evidence_type"], "structural");
}
