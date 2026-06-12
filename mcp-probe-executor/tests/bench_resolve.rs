use std::process::Command;
use std::time::Instant;

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
fn bench_eight_hypotheses_complete_and_valid_json() {
    let f = fixtures();
    let start = Instant::now();

    let out = Command::new(binary())
        .args([
            "--hypotheses", f.join("hypset_eight.json").to_str().unwrap(),
            "--budget",     f.join("budget_unlimited_large.json").to_str().unwrap(),
            "--summaries",  f.join("summaries8").to_str().unwrap(),
            "--baseline",   f.join("summaries8/baseline").to_str().unwrap(),
            "--now-ms",     "1000000",
            "--format",     "json",
        ])
        .output()
        .expect("failed to run binary");

    let elapsed = start.elapsed();

    assert!(
        out.status.success(),
        "binary failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Should complete well under 300ms (this is a file-read test mode, no network)
    assert!(
        elapsed.as_millis() < 300,
        "took {}ms, expected < 300ms",
        elapsed.as_millis()
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("not valid JSON: {}\nstdout: {}", e, stdout));

    // All 8 hypotheses should be in resolved
    let resolved = v["resolved"].as_array().expect("resolved is array");
    assert_eq!(resolved.len(), 8, "expected 8 resolved entries");

    // evidence_type is always structural
    assert_eq!(v["evidence_type"], "structural");
}

#[test]
fn bench_deterministic_with_fixed_now_ms() {
    // Run twice with the same --now-ms; output must be identical
    let f = fixtures();

    let run = |now_ms: &str| {
        Command::new(binary())
            .args([
                "--hypotheses", f.join("hypset_eight.json").to_str().unwrap(),
                "--budget",     f.join("budget_unlimited_large.json").to_str().unwrap(),
                "--summaries",  f.join("summaries8").to_str().unwrap(),
                "--baseline",   f.join("summaries8/baseline").to_str().unwrap(),
                "--now-ms",     now_ms,
                "--format",     "json",
            ])
            .output()
            .expect("failed to run binary")
    };

    let out1 = run("5000000");
    let out2 = run("5000000");

    assert!(out1.status.success());
    assert!(out2.status.success());

    let s1 = String::from_utf8_lossy(&out1.stdout);
    let s2 = String::from_utf8_lossy(&out2.stdout);

    assert_eq!(s1, s2, "output is not deterministic with fixed --now-ms");
}
