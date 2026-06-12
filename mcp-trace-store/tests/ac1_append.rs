//! AC1: `append` on a new store creates the JSONL file and writes a valid JSON
//! object on the first line. A second `append` adds a second valid JSON line.
use mcp_trace_store::{BindOutcome, ExecuteOutcome, QualitySignals, TraceRecord, TraceStore, TraceStoreConfig};
use std::fs;
use tempfile::TempDir;

fn make_record(session: &str) -> TraceRecord {
    TraceRecord::new(
        session,
        serde_json::json!({"entity": "Revenue", "metric": "SalesAmount"}),
        BindOutcome::Success,
        ExecuteOutcome::Success { row_count: 10, result_empty: false },
        QualitySignals {
            first_attempt_bind: true,
            bind_attempt_count: 1,
            total_latency_ms: 42,
            tokens_used: None,
        },
    )
}

fn make_store(dir: &TempDir) -> TraceStore {
    let cfg = TraceStoreConfig::new(dir.path().join("trace.jsonl"));
    TraceStore::new(cfg).unwrap()
}

#[test]
fn ac1_creates_file_on_first_append() {
    let dir = TempDir::new().unwrap();
    let store = make_store(&dir);
    let path = dir.path().join("trace.jsonl");

    assert!(!path.exists(), "file should not exist before first append");

    let returned = store.append(make_record("sess-ac1")).unwrap();
    assert!(!returned.record_id.is_empty());
    assert!(returned.timestamp_ms > 0);

    assert!(path.exists(), "file should exist after first append");

    let contents = fs::read_to_string(&path).unwrap();
    let first_line = contents.lines().next().unwrap();
    let _: serde_json::Value = serde_json::from_str(first_line)
        .expect("first line must be valid JSON");
}

#[test]
fn ac1_second_append_adds_second_line() {
    let dir = TempDir::new().unwrap();
    let store = make_store(&dir);
    let path = dir.path().join("trace.jsonl");

    store.append(make_record("sess-ac1-a")).unwrap();
    store.append(make_record("sess-ac1-b")).unwrap();

    let contents = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = contents.lines().collect();
    assert_eq!(lines.len(), 2, "should have exactly two lines");

    for line in &lines {
        let _: serde_json::Value = serde_json::from_str(line)
            .expect("each line must be valid JSON");
    }
}

#[test]
fn ac1_returned_record_has_populated_ids() {
    let dir = TempDir::new().unwrap();
    let store = make_store(&dir);

    let mut rec = make_record("sess-ac1-ids");
    rec.record_id = String::new(); // clear so store mints it
    rec.timestamp_ms = 0;

    let returned = store.append(rec).unwrap();
    assert!(!returned.record_id.is_empty(), "record_id should be minted");
    assert!(returned.timestamp_ms > 0, "timestamp_ms should be set");
}
