//! AC3: `scan` with `first_attempt_only: true` returns only records where
//! `quality.first_attempt_bind == true`.
use mcp_trace_store::{
    BindOutcome, ExecuteOutcome, QualitySignals, TraceFilter, TraceRecord, TraceStore,
    TraceStoreConfig,
};
use tempfile::TempDir;

fn record_with_first_attempt(session: &str, first_attempt: bool, attempts: u8) -> TraceRecord {
    TraceRecord::new(
        session,
        serde_json::json!({}),
        BindOutcome::Success,
        ExecuteOutcome::Success { row_count: 5, result_empty: false },
        QualitySignals {
            first_attempt_bind: first_attempt,
            bind_attempt_count: attempts,
            total_latency_ms: 20,
            tokens_used: None,
        },
    )
}

#[test]
fn ac3_first_attempt_filter() {
    let dir = TempDir::new().unwrap();
    let cfg = TraceStoreConfig::new(dir.path().join("trace.jsonl"));
    let store = TraceStore::new(cfg).unwrap();

    // First attempt succeeded
    store.append(record_with_first_attempt("sess-ac3", true, 1)).unwrap();
    // Needed retries
    store.append(record_with_first_attempt("sess-ac3", false, 3)).unwrap();
    // Another first-attempt success
    store.append(record_with_first_attempt("sess-ac3", true, 1)).unwrap();

    let filter = TraceFilter {
        first_attempt_only: true,
        ..TraceFilter::default()
    };

    let results = store.scan(&filter).unwrap();
    assert_eq!(results.len(), 2, "should return only first-attempt-bind records");
    for r in &results {
        assert!(r.quality.first_attempt_bind);
    }
}

#[test]
fn ac3_no_filter_returns_all() {
    let dir = TempDir::new().unwrap();
    let cfg = TraceStoreConfig::new(dir.path().join("trace.jsonl"));
    let store = TraceStore::new(cfg).unwrap();

    store.append(record_with_first_attempt("sess-ac3-all", true, 1)).unwrap();
    store.append(record_with_first_attempt("sess-ac3-all", false, 2)).unwrap();

    let filter = TraceFilter::default();
    let results = store.scan(&filter).unwrap();
    assert_eq!(results.len(), 2);
}
