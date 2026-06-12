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
fn ac5_missing_summary_is_inconclusive_not_fatal() {
    // hypset_with_missing.json has rank=1 with probe_key="missing-probe-key-xyz" (no file)
    // and rank=2 with probe_key="store-sales-amount" (file exists)
    let f = fixtures();
    let out = Command::new(binary())
        .args([
            "--hypotheses", f.join("hypset_with_missing.json").to_str().unwrap(),
            "--budget",     f.join("budget_unlimited.json").to_str().unwrap(),
            "--summaries",  f.join("summaries").to_str().unwrap(),
            "--baseline",   f.join("summaries/baseline").to_str().unwrap(),
            "--now-ms",     "1000000",
            "--format",     "json",
        ])
        .output()
        .expect("failed to run binary");

    // Must exit successfully (not abort)
    assert!(
        out.status.success(),
        "binary should not abort on missing summary file; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("not valid JSON: {}\nstdout: {}", e, stdout));

    let resolved = v["resolved"].as_array().expect("resolved is array");
    assert_eq!(resolved.len(), 2);

    // rank=1 (missing file) → inconclusive
    let r1 = &resolved[0];
    assert_eq!(r1["rank"], 1);
    assert_eq!(
        r1["verdict"], "inconclusive",
        "rank 1 should be inconclusive for missing probe file"
    );

    // rank=2 (file exists) → should resolve normally
    let r2 = &resolved[1];
    assert_eq!(r2["rank"], 2);
    assert_ne!(
        r2["verdict"], "inconclusive",
        "rank 2 should resolve normally, got: {}",
        r2["verdict"]
    );

    // stderr should mention the missing key
    assert!(
        stderr.contains("missing-probe-key-xyz") || stderr.contains("inconclusive"),
        "expected stderr to mention the missing probe key; got: {}",
        stderr
    );
}
