use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_mcp-investigation-orchestrator")
}

fn write_fixture_inputs(dir: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let watch = dir.path().join("watch-event.json");
    let describe = dir.path().join("describe-model.json");
    fs::write(
        &watch,
        r#"{
          "query_id": "standing-query-1",
          "measure": "[Measures].[Total Store Sales]",
          "prior": 100.0,
          "observed": 90.0
        }"#,
    )
    .unwrap();
    fs::write(
        &describe,
        r#"{
          "name": "fixture",
          "measures": [
            {"name": "Store Sales Amount", "unique_name": "[Measures].[Store Sales Amount]"},
            {"name": "Store Cost", "unique_name": "[Measures].[Store Cost]"}
          ],
          "calculated_members": [
            {
              "name": "Total Store Sales",
              "unique_name": "[Measures].[Total Store Sales]",
              "expression": "[Measures].[Store Sales Amount] + [Measures].[Store Cost]"
            }
          ]
        }"#,
    )
    .unwrap();
    (watch, describe)
}

fn mock_probe(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let path = dir.path().join("mock-probe-executor");
    fs::write(
        &path,
        r#"#!/bin/sh
printf '%s\n' '{"target":"[Measures].[Total Store Sales]","resolved":[{"rank":1,"verdict":"confirmed","observed_delta_fraction":-0.1}],"budget":{"queries_run":1}}'
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

#[test]
fn ac1_happy_path() {
    let dir = tempfile::tempdir().unwrap();
    let (watch, describe) = write_fixture_inputs(&dir);
    let probe = mock_probe(&dir);
    let store = dir.path().join("store");

    let out = Command::new(binary())
        .args([
            "--watch-event",
            watch.to_str().unwrap(),
            "--describe-model",
            describe.to_str().unwrap(),
            "--finding-store",
            store.to_str().unwrap(),
            "--probe-executor",
            probe.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let findings: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!findings.is_empty());
    assert!(findings.iter().any(|f| {
        f["status"] == "Confirmed" || f["status"] == "Refuted"
    }));
}
