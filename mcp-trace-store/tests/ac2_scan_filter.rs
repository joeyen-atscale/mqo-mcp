//! AC2: `scan` with `grounding_band: Some(Ungroundable)` returns only records
//! where `grounding_band == Ungroundable`. Records with `None` grounding_band
//! are excluded.
use mcp_trace_store::{
    BindOutcome, ExecuteOutcome, GroundingBand, QualitySignals, TraceFilter, TraceRecord,
    TraceStore, TraceStoreConfig,
};
use tempfile::TempDir;

fn base_record(session: &str) -> TraceRecord {
    TraceRecord::new(
        session,
        serde_json::json!({}),
        BindOutcome::Success,
        ExecuteOutcome::Success { row_count: 1, result_empty: false },
        QualitySignals {
            first_attempt_bind: true,
            bind_attempt_count: 1,
            total_latency_ms: 10,
            tokens_used: None,
        },
    )
}

#[test]
fn ac2_grounding_band_filter() {
    let dir = TempDir::new().unwrap();
    let cfg = TraceStoreConfig::new(dir.path().join("trace.jsonl"));
    let store = TraceStore::new(cfg).unwrap();

    // Grounded record
    let mut r1 = base_record("sess-ac2");
    r1.grounding_band = Some(GroundingBand::Grounded);
    store.append(r1).unwrap();

    // Partial record
    let mut r2 = base_record("sess-ac2");
    r2.grounding_band = Some(GroundingBand::Partial);
    store.append(r2).unwrap();

    // Ungroundable record
    let mut r3 = base_record("sess-ac2");
    r3.grounding_band = Some(GroundingBand::Ungroundable);
    store.append(r3).unwrap();

    // Record with no grounding_band
    let r4 = base_record("sess-ac2");
    store.append(r4).unwrap();

    let filter = TraceFilter {
        grounding_band: Some(GroundingBand::Ungroundable),
        ..TraceFilter::default()
    };

    let results = store.scan(&filter).unwrap();
    assert_eq!(results.len(), 1, "should return only the Ungroundable record");
    assert_eq!(
        results[0].grounding_band,
        Some(GroundingBand::Ungroundable)
    );
}

#[test]
fn ac2_none_grounding_band_excluded() {
    let dir = TempDir::new().unwrap();
    let cfg = TraceStoreConfig::new(dir.path().join("trace.jsonl"));
    let store = TraceStore::new(cfg).unwrap();

    // Two records with None grounding_band
    store.append(base_record("sess-ac2-none-a")).unwrap();
    store.append(base_record("sess-ac2-none-b")).unwrap();

    let filter = TraceFilter {
        grounding_band: Some(GroundingBand::Ungroundable),
        ..TraceFilter::default()
    };

    let results = store.scan(&filter).unwrap();
    assert!(results.is_empty(), "None grounding_band records should be excluded");
}
