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

const VALID_VERDICTS: &[&str] = &["confirmed", "refuted", "inconclusive", "skipped_budget"];
const REQUIRED_ANALYSIS_NOTE: &str =
    "Probes executed under budget; confirm/refute is a directional data check, not statistical causation.";

#[test]
fn ac4_schema_evidence_type_and_verdicts() {
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

    // evidence_type must be "structural"
    assert_eq!(
        v["evidence_type"].as_str().unwrap_or(""),
        "structural",
        "evidence_type must be 'structural'"
    );

    // analysis_note must be present and non-empty
    let note = v["analysis_note"].as_str().unwrap_or("");
    assert!(!note.is_empty(), "analysis_note must not be empty");
    assert_eq!(note, REQUIRED_ANALYSIS_NOTE, "analysis_note must match verbatim");

    // Every resolved entry must have verdict in the allowed set
    let resolved = v["resolved"].as_array().expect("resolved is array");
    for (i, r) in resolved.iter().enumerate() {
        let verdict = r["verdict"].as_str().unwrap_or("");
        assert!(
            VALID_VERDICTS.contains(&verdict),
            "resolved[{}] has invalid verdict: '{}'",
            i,
            verdict
        );
    }
}

#[test]
fn ac4_schema_halted_set_also_valid() {
    // Run with max_queries=1 to trigger halt and skipped_budget
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

    assert_eq!(v["evidence_type"].as_str().unwrap_or(""), "structural");
    assert!(!v["analysis_note"].as_str().unwrap_or("").is_empty());

    let resolved = v["resolved"].as_array().expect("resolved is array");
    for (i, r) in resolved.iter().enumerate() {
        let verdict = r["verdict"].as_str().unwrap_or("");
        assert!(
            VALID_VERDICTS.contains(&verdict),
            "resolved[{}] has invalid verdict: '{}'",
            i,
            verdict
        );
    }
}
