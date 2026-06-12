//! AC6: A record with a missing optional field (e.g. `grounding_score` not
//! present in the JSONL) deserializes successfully with `grounding_score: None`.
//! Forward compatibility: a record with an unknown field is deserialized
//! without error (extra fields ignored).
use mcp_trace_store::{TraceFilter, TraceStore, TraceStoreConfig};
use std::fs::OpenOptions;
use std::io::Write;
use tempfile::TempDir;

/// Minimal valid record — omits optional fields (`grounding_score`,
/// `grounding_band`, `cluster_name`, `tokens_used`).
const MINIMAL_RECORD: &str = r#"{"record_id":"rec-minimal","session_id":"sess-ac6","timestamp_ms":1700000000000,"mqo":{},"bind_outcome":{"type":"success"},"execute_result":{"type":"success","row_count":1,"result_empty":false},"quality":{"first_attempt_bind":true,"bind_attempt_count":1,"total_latency_ms":10}}"#;

/// Record with an unknown field (`future_field`) that didn't exist in this version.
const FORWARD_COMPAT_RECORD: &str = r#"{"record_id":"rec-future","session_id":"sess-ac6","timestamp_ms":1700000001000,"mqo":{},"bind_outcome":{"type":"success"},"execute_result":{"type":"skipped"},"quality":{"first_attempt_bind":false,"bind_attempt_count":2,"total_latency_ms":200,"tokens_used":500},"future_field":"should_be_ignored","another_new_field":{"nested":true}}"#;

fn write_lines(path: &std::path::Path, lines: &[&str]) {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    for line in lines {
        writeln!(f, "{}", line).unwrap();
    }
}

#[test]
fn ac6_missing_optional_fields_deserialize_as_none() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("trace.jsonl");
    write_lines(&path, &[MINIMAL_RECORD]);

    let cfg = TraceStoreConfig::new(&path);
    let store = TraceStore::new(cfg).unwrap();

    let results = store.scan(&TraceFilter::default()).unwrap();
    assert_eq!(results.len(), 1);

    let r = &results[0];
    assert_eq!(r.record_id, "rec-minimal");
    assert!(r.grounding_score.is_none(), "grounding_score should be None when absent");
    assert!(r.grounding_band.is_none(), "grounding_band should be None when absent");
    assert!(r.cluster_name.is_none(), "cluster_name should be None when absent");
    assert!(r.quality.tokens_used.is_none(), "tokens_used should be None when absent");
}

#[test]
fn ac6_unknown_fields_ignored() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("trace.jsonl");
    write_lines(&path, &[FORWARD_COMPAT_RECORD]);

    let cfg = TraceStoreConfig::new(&path);
    let store = TraceStore::new(cfg).unwrap();

    let results = store.scan(&TraceFilter::default()).unwrap();
    assert_eq!(results.len(), 1, "record with unknown fields should deserialize without error");
    assert_eq!(results[0].record_id, "rec-future");
    assert_eq!(results[0].quality.tokens_used, Some(500));
}

#[test]
fn ac6_corrupt_line_skipped_good_lines_returned() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("trace.jsonl");
    write_lines(&path, &[
        MINIMAL_RECORD,
        "{not valid json at all!!!",
        FORWARD_COMPAT_RECORD,
    ]);

    let cfg = TraceStoreConfig::new(&path);
    let store = TraceStore::new(cfg).unwrap();

    let results = store.scan(&TraceFilter::default()).unwrap();
    assert_eq!(results.len(), 2, "should skip corrupt line and return 2 valid records");
    assert_eq!(results[0].record_id, "rec-minimal");
    assert_eq!(results[1].record_id, "rec-future");
}
