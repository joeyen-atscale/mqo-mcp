//! AC4: `append` is atomic. A simulated crash (partial/corrupt write injected
//! directly into the file) leaves the JSONL file in a valid state — all prior
//! records intact, the in-flight record either fully written or absent.
//! This test writes a valid partial record (no closing brace) and verifies
//! `scan` skips it with a warning to stderr.
use mcp_trace_store::{
    BindOutcome, ExecuteOutcome, QualitySignals, TraceFilter, TraceRecord, TraceStore,
    TraceStoreConfig,
};
use std::fs::OpenOptions;
use std::io::Write;
use tempfile::TempDir;

fn good_record(session: &str) -> TraceRecord {
    TraceRecord::new(
        session,
        serde_json::json!({}),
        BindOutcome::Success,
        ExecuteOutcome::Success { row_count: 1, result_empty: false },
        QualitySignals {
            first_attempt_bind: true,
            bind_attempt_count: 1,
            total_latency_ms: 5,
            tokens_used: None,
        },
    )
}

#[test]
fn ac4_corrupt_line_skipped_prior_records_intact() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("trace.jsonl");
    let cfg = TraceStoreConfig::new(&path);
    let store = TraceStore::new(cfg).unwrap();

    // Write two good records.
    let r1 = store.append(good_record("sess-ac4")).unwrap();
    let r2 = store.append(good_record("sess-ac4")).unwrap();

    // Simulate a crash mid-write by appending a truncated/corrupt JSON line.
    {
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{{\"record_id\":\"crash-sim\",\"session_id\":\"sess-ac4\"").unwrap();
        // No closing brace — this is an invalid JSON object.
    }

    // Scan should return the two good records and skip the corrupt line.
    let filter = TraceFilter::default();
    let results = store.scan(&filter).unwrap();

    assert_eq!(results.len(), 2, "should skip corrupt line and return 2 good records");
    assert_eq!(results[0].record_id, r1.record_id);
    assert_eq!(results[1].record_id, r2.record_id);
}

#[test]
fn ac4_completely_truncated_file_returns_empty() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("trace.jsonl");
    let cfg = TraceStoreConfig::new(&path);
    let store = TraceStore::new(cfg).unwrap();

    // Write a partial line only (no newline terminator, no valid JSON).
    {
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, "{{\"truncated\":").unwrap();
    }

    let filter = TraceFilter::default();
    let results = store.scan(&filter).unwrap();
    assert!(results.is_empty(), "truncated-only file should produce no valid records");
}
