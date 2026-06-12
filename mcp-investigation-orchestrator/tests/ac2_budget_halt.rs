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
        r#"{"query_id":"budget-query","measure":"[Measures].[Total Store Sales]","prior":100.0,"observed":90.0}"#,
    )
    .unwrap();
    fs::write(
        &describe,
        r#"{
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

fn counting_probe(dir: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let counter = dir.path().join("probe-count");
    let path = dir.path().join("counting-probe-executor");
    fs::write(
        &path,
        format!(
            r#"#!/bin/sh
count_file='{}'
count=0
if [ -f "$count_file" ]; then count=$(cat "$count_file"); fi
count=$((count + 1))
printf '%s' "$count" > "$count_file"
printf '%s\n' '{{"target":"x","resolved":[{{"rank":1,"verdict":"confirmed","observed_delta_fraction":-0.1}}],"budget":{{"queries_run":1}}}}'
"#,
            counter.display()
        ),
    )
    .unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap();
    (path, counter)
}

#[test]
fn ac2_budget_halt() {
    let dir = tempfile::tempdir().unwrap();
    let (watch, describe) = write_fixture_inputs(&dir);
    let (probe, counter) = counting_probe(&dir);
    let store = dir.path().join("store");

    let out = Command::new(binary())
        .args([
            "--watch-event",
            watch.to_str().unwrap(),
            "--describe-model",
            describe.to_str().unwrap(),
            "--finding-store",
            store.to_str().unwrap(),
            "--budget-max-queries",
            "1",
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
    assert_eq!(fs::read_to_string(counter).unwrap(), "1");
    let findings: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
    let resolved = findings[0]["resolved_hypotheses"]["resolved"]
        .as_array()
        .unwrap();
    let notes = findings[0]["resolved_hypotheses"]["notes"].to_string();
    assert!(
        notes.contains("budget_halt") || resolved.len() < 2,
        "expected budget halt note or fewer findings than hypotheses"
    );
}
