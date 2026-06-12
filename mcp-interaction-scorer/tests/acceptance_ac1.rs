//! AC1: per-session retry_rate, empty_result_rate, bind_failure_rate.
//!
//! Fixture: 3 sessions with known counts; assert computed rates match expected
//! f64 within 1e-9.

use mcp_interaction_scorer::score_reader;
use mcp_trace_store::{BindOutcome, ExecuteOutcome, QualitySignals, TraceRecord};
use serde_json::json;

fn record(
    session_id: &str,
    bind_attempt_count: u8,
    first_attempt_bind: bool,
    bind_outcome: BindOutcome,
    execute_result: ExecuteOutcome,
) -> TraceRecord {
    TraceRecord {
        record_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_owned(),
        cluster_name: None,
        timestamp_ms: 1_000,
        mqo: json!({"measures": [{"unique_name": "sales.revenue"}]}),
        bind_outcome,
        grounding_score: None,
        grounding_band: None,
        execute_result,
        quality: QualitySignals {
            first_attempt_bind,
            bind_attempt_count,
            total_latency_ms: 50,
            tokens_used: None,
        },
        user_question: None,
    }
}

fn to_jsonl(records: &[TraceRecord]) -> Vec<u8> {
    records
        .iter()
        .flat_map(|r| {
            let mut s = serde_json::to_string(r).unwrap();
            s.push('\n');
            s.into_bytes()
        })
        .collect()
}

#[test]
fn ac1_per_session_rates() {
    // session A: 4 MQOs
    //   - 2 retried (bind_attempt_count > 1)
    //   - 1 empty result
    //   - 1 bind failure (NotFound)
    let session_a = vec![
        record("A", 1, true, BindOutcome::Success, ExecuteOutcome::Success { row_count: 5, result_empty: false }),
        record("A", 2, false, BindOutcome::Success, ExecuteOutcome::Success { row_count: 0, result_empty: true }),
        record("A", 3, false, BindOutcome::Success, ExecuteOutcome::Success { row_count: 2, result_empty: false }),
        record("A", 1, true, BindOutcome::NotFound, ExecuteOutcome::Skipped),
    ];

    // session B: 2 MQOs
    //   - 0 retried
    //   - 0 empty results
    //   - 0 bind failures
    let session_b = vec![
        record("B", 1, true, BindOutcome::Success, ExecuteOutcome::Success { row_count: 10, result_empty: false }),
        record("B", 1, true, BindOutcome::Success, ExecuteOutcome::Success { row_count: 3, result_empty: false }),
    ];

    // session C: 3 MQOs
    //   - 3 retried (all have bind_attempt_count > 1)
    //   - 2 empty results
    //   - 2 bind failures (Ambiguous)
    let session_c = vec![
        record("C", 2, false, BindOutcome::Ambiguous, ExecuteOutcome::Skipped),
        record("C", 3, false, BindOutcome::Ambiguous, ExecuteOutcome::Skipped),
        record("C", 2, false, BindOutcome::Success, ExecuteOutcome::Success { row_count: 0, result_empty: true }),
    ];

    let mut all = session_a;
    all.extend(session_b);
    all.extend(session_c);

    let jsonl = to_jsonl(&all);
    let report = score_reader(std::io::Cursor::new(jsonl)).unwrap();

    // ── Session A ──
    let a = &report.sessions["A"];
    assert_eq!(a.total_mqos, 4);
    // retried: 2 (bind_attempt_count > 1 for records 2 and 3)
    assert!((a.retry_rate - 2.0 / 4.0).abs() < 1e-9, "A retry_rate={}", a.retry_rate);
    // empty result: 1
    assert!((a.empty_result_rate - 1.0 / 4.0).abs() < 1e-9, "A empty_result_rate={}", a.empty_result_rate);
    // bind failure: 1 (NotFound)
    assert!((a.bind_failure_rate - 1.0 / 4.0).abs() < 1e-9, "A bind_failure_rate={}", a.bind_failure_rate);

    // ── Session B ──
    let b = &report.sessions["B"];
    assert_eq!(b.total_mqos, 2);
    assert!((b.retry_rate - 0.0).abs() < 1e-9);
    assert!((b.empty_result_rate - 0.0).abs() < 1e-9);
    assert!((b.bind_failure_rate - 0.0).abs() < 1e-9);

    // ── Session C ──
    let c = &report.sessions["C"];
    assert_eq!(c.total_mqos, 3);
    // all 3 retried
    assert!((c.retry_rate - 3.0 / 3.0).abs() < 1e-9, "C retry_rate={}", c.retry_rate);
    // 1 empty result (the last Success with result_empty: true)
    assert!((c.empty_result_rate - 1.0 / 3.0).abs() < 1e-9, "C empty_result_rate={}", c.empty_result_rate);
    // 2 bind failures (both Ambiguous)
    assert!((c.bind_failure_rate - 2.0 / 3.0).abs() < 1e-9, "C bind_failure_rate={}", c.bind_failure_rate);
}
