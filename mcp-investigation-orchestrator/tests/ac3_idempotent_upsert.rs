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
        r#"{"query_id":"same-query","measure":"[Measures].[Total Store Sales]","prior":100.0,"observed":90.0}"#,
    )
    .unwrap();
    fs::write(
        &describe,
        r#"{
          "measures": [
            {"name": "Store Sales Amount", "unique_name": "[Measures].[Store Sales Amount]"}
          ],
          "calculated_members": [
            {
              "name": "Total Store Sales",
              "unique_name": "[Measures].[Total Store Sales]",
              "expression": "[Measures].[Store Sales Amount]"
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
printf '%s\n' '{"target":"x","resolved":[{"rank":1,"verdict":"refuted","observed_delta_fraction":0.1}],"budget":{"queries_run":1}}'
"#,
    )
    .unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    path
}

#[test]
fn ac3_idempotent_upsert() {
    let dir = tempfile::tempdir().unwrap();
    let (watch, describe) = write_fixture_inputs(&dir);
    let probe = mock_probe(&dir);
    let store = dir.path().join("store");

    let run_once = || {
        Command::new(binary())
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
            .unwrap()
    };

    let first = run_once();
    assert!(first.status.success());
    let first_findings: Vec<Value> = serde_json::from_slice(&first.stdout).unwrap();
    let first_id = first_findings[0]["finding_id"].as_str().unwrap().to_string();

    let second = run_once();
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let second_findings: Vec<Value> = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(second_findings.len(), 1);
    assert_eq!(second_findings[0]["finding_id"], first_id);
    assert_eq!(second_findings[0]["query_id"], "same-query");
    assert_eq!(second_findings[0]["recurrence_count"], 1);
}
