//! Integration tests for the `mqo-chart-recommender` binary.
//!
//! Invokes the binary via `std::process::Command` and asserts on stdout /
//! exit codes. Guards against dispatch regressions that lib-only tests can't
//! catch.

use std::process::Command;

fn binary() -> Command {
    let bin = env!("CARGO_BIN_EXE_mqo-chart-recommender");
    Command::new(bin)
}

fn fixture_path(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn cli_json_output_for_temporal() {
    let out = binary()
        .args(["--profile", &fixture_path("profile_temporal.json")])
        .output()
        .expect("binary should run");

    assert!(out.status.success(), "exit code: {}", out.status);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let val: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout must be valid JSON");

    assert_eq!(
        val["mark"].as_str(),
        Some("line"),
        "mark should be 'line'"
    );
    assert_eq!(val["schema"].as_str(), Some("chart-recommendation.v1"));
}

#[test]
fn cli_human_format_for_kpi() {
    let out = binary()
        .args([
            "--profile",
            &fixture_path("profile_kpi.json"),
            "--format",
            "human",
        ])
        .output()
        .expect("binary should run");

    assert!(out.status.success(), "exit code: {}", out.status);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("BigNumber"),
        "human output must contain 'BigNumber'; got: {stdout}"
    );
}

#[test]
fn cli_exits_nonzero_on_missing_file() {
    let out = binary()
        .args(["--profile", "/nonexistent/path/profile.json"])
        .output()
        .expect("binary should run");

    assert!(
        !out.status.success(),
        "binary must exit non-zero for missing file"
    );
}
