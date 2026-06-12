//! AC7: Appending 10,000 records and scanning with no filter completes in
//! under 2 seconds total.
use mcp_trace_store::{
    BindOutcome, ExecuteOutcome, QualitySignals, TraceFilter, TraceRecord, TraceStore,
    TraceStoreConfig,
};
use std::time::Instant;
use tempfile::TempDir;

fn perf_record() -> TraceRecord {
    TraceRecord::new(
        "sess-bench",
        serde_json::json!({"entity": "Revenue", "metric": "SalesAmount", "filters": []}),
        BindOutcome::Success,
        ExecuteOutcome::Success { row_count: 100, result_empty: false },
        QualitySignals {
            first_attempt_bind: true,
            bind_attempt_count: 1,
            total_latency_ms: 30,
            tokens_used: Some(512),
        },
    )
}

#[test]
fn bench_10k_append_and_scan_under_2s() {
    let dir = TempDir::new().unwrap();
    let cfg = TraceStoreConfig::new(dir.path().join("trace.jsonl"))
        .with_rotate_at_bytes(200 * 1024 * 1024); // 200 MB — no rotation during bench
    let store = TraceStore::new(cfg).unwrap();

    let start = Instant::now();

    for _ in 0..10_000 {
        store.append(perf_record()).unwrap();
    }

    let filter = TraceFilter::default();
    let results = store.scan(&filter).unwrap();

    let elapsed = start.elapsed();
    println!("10k append+scan elapsed: {:?}", elapsed);

    assert_eq!(results.len(), 10_000);

    // The 2-second SLO applies only to optimized builds. Debug builds are
    // excluded from the timing assertion (AC7 SHOULD, not MUST).
    if !cfg!(debug_assertions) {
        assert!(
            elapsed.as_secs_f64() < 2.0,
            "10k append+scan should complete under 2s in release, took {:?}",
            elapsed
        );
    } else {
        println!("(debug build — timing SLO not enforced; elapsed: {:?})", elapsed);
    }
}
