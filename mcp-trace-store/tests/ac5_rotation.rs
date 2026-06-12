//! AC5: When the store file exceeds `rotate_at_bytes`, the next `append`
//! writes to the active file (the old active becomes `.1`). `scan` reads `.1`
//! (oldest) then active and returns records from both in order.
use mcp_trace_store::{
    BindOutcome, ExecuteOutcome, QualitySignals, TraceFilter, TraceRecord, TraceStore,
    TraceStoreConfig,
};
use tempfile::TempDir;

fn record(session: &str) -> TraceRecord {
    TraceRecord::new(
        session,
        serde_json::json!({"entity": "Sales"}),
        BindOutcome::Success,
        ExecuteOutcome::Success { row_count: 42, result_empty: false },
        QualitySignals {
            first_attempt_bind: true,
            bind_attempt_count: 1,
            total_latency_ms: 15,
            tokens_used: None,
        },
    )
}

#[test]
fn ac5_rotation_splits_files() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("trace.jsonl");

    // Set a tiny rotation threshold (1 byte) so rotation triggers immediately
    // after the first record.
    let cfg = TraceStoreConfig::new(&path).with_rotate_at_bytes(1);
    let store = TraceStore::new(cfg).unwrap();

    let r1 = store.append(record("sess-ac5")).unwrap();
    // After r1, file > 1 byte → next append should rotate.
    let r2 = store.append(record("sess-ac5")).unwrap();

    // .1 should exist (old active).
    let rotated = dir.path().join("trace.jsonl.1");
    assert!(rotated.exists(), "rotated file trace.jsonl.1 should exist");

    // Active file should also exist.
    assert!(path.exists(), "active file should exist after second append");

    // scan should return both records in order (oldest first = from .1 then active).
    let filter = TraceFilter::default();
    let results = store.scan(&filter).unwrap();
    assert_eq!(results.len(), 2, "scan should return records from both fragments");

    // First result should be r1 (from the older rotated file).
    assert_eq!(results[0].record_id, r1.record_id);
    assert_eq!(results[1].record_id, r2.record_id);
}

#[test]
fn ac5_no_rotation_below_threshold() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("trace.jsonl");

    // Large threshold — no rotation should happen.
    let cfg = TraceStoreConfig::new(&path).with_rotate_at_bytes(100 * 1024 * 1024);
    let store = TraceStore::new(cfg).unwrap();

    store.append(record("sess-ac5-nrot")).unwrap();
    store.append(record("sess-ac5-nrot")).unwrap();

    let rotated = dir.path().join("trace.jsonl.1");
    assert!(!rotated.exists(), "no rotation should occur below threshold");

    let results = store.scan(&TraceFilter::default()).unwrap();
    assert_eq!(results.len(), 2);
}
