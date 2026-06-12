//! AC2: per-entity distributions: first_attempt_bind_rate, retry_histogram,
//! result_fill_rate, grounding_score distribution.
//!
//! Fixture: 2 named entities; assert per-entity stats match expected values.

use mcp_interaction_scorer::score_reader;
use mcp_trace_store::{BindOutcome, ExecuteOutcome, QualitySignals, TraceRecord};
use serde_json::json;

fn record_with_entities(
    entities: serde_json::Value,
    bind_attempt_count: u8,
    first_attempt_bind: bool,
    execute_result: ExecuteOutcome,
    grounding_score: Option<f64>,
) -> TraceRecord {
    TraceRecord {
        record_id: uuid::Uuid::new_v4().to_string(),
        session_id: "s1".to_owned(),
        cluster_name: None,
        timestamp_ms: 1_000,
        mqo: entities,
        bind_outcome: BindOutcome::Success,
        grounding_score,
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
fn ac2_per_entity_stats() {
    // Entity "revenue" appears in 3 records.
    // Entity "region" appears in 2 records.
    let records = vec![
        // Record 1: revenue + region, first attempt, filled
        record_with_entities(
            json!({"measures": [{"unique_name": "revenue"}], "dimensions": [{"unique_name": "region"}]}),
            1, true,
            ExecuteOutcome::Success { row_count: 5, result_empty: false },
            Some(0.9),
        ),
        // Record 2: revenue only, retried, empty
        record_with_entities(
            json!({"measures": [{"unique_name": "revenue"}]}),
            2, false,
            ExecuteOutcome::Success { row_count: 0, result_empty: true },
            Some(0.5),
        ),
        // Record 3: revenue only, retried, filled
        record_with_entities(
            json!({"measures": [{"unique_name": "revenue"}]}),
            3, false,
            ExecuteOutcome::Success { row_count: 7, result_empty: false },
            None,
        ),
        // Record 4: region only, retried, empty
        record_with_entities(
            json!({"dimensions": [{"unique_name": "region"}]}),
            2, false,
            ExecuteOutcome::Success { row_count: 0, result_empty: true },
            Some(0.3),
        ),
    ];

    let jsonl = to_jsonl(&records);
    let report = score_reader(std::io::Cursor::new(jsonl)).unwrap();

    // ── Entity "revenue" — 3 interactions ──
    let rev = report.entities.get("revenue").expect("revenue entity missing");
    assert_eq!(rev.total_interactions, 3, "revenue total_interactions");
    // first attempt succeeded in record 1 only => 1/3
    assert!(
        (rev.first_attempt_bind_rate - 1.0 / 3.0).abs() < 1e-9,
        "revenue first_attempt_bind_rate={}", rev.first_attempt_bind_rate
    );
    // retry histogram: [1, 2, 3]
    let mut hist = rev.retry_histogram.clone();
    hist.sort();
    assert_eq!(hist, vec![1, 2, 3], "revenue retry_histogram");
    // filled: records 1 and 3 => 2/3
    assert!(
        (rev.result_fill_rate - 2.0 / 3.0).abs() < 1e-9,
        "revenue result_fill_rate={}", rev.result_fill_rate
    );
    // grounding scores: [0.9, 0.5] (record 3 had None)
    let mut scores = rev.grounding_scores.clone();
    scores.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(scores.len(), 2);
    assert!((scores[0] - 0.5).abs() < 1e-9);
    assert!((scores[1] - 0.9).abs() < 1e-9);

    // ── Entity "region" — 2 interactions ──
    let reg = report.entities.get("region").expect("region entity missing");
    assert_eq!(reg.total_interactions, 2, "region total_interactions");
    // first attempt: record 1 only => 1/2
    assert!(
        (reg.first_attempt_bind_rate - 0.5).abs() < 1e-9,
        "region first_attempt_bind_rate={}", reg.first_attempt_bind_rate
    );
    // histogram: [1, 2]
    let mut rhist = reg.retry_histogram.clone();
    rhist.sort();
    assert_eq!(rhist, vec![1, 2]);
    // filled: record 1 only => 1/2
    assert!(
        (reg.result_fill_rate - 0.5).abs() < 1e-9,
        "region result_fill_rate={}", reg.result_fill_rate
    );
    // grounding scores: [0.9, 0.3]
    let mut rscores = reg.grounding_scores.clone();
    rscores.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(rscores.len(), 2);
    assert!((rscores[0] - 0.3).abs() < 1e-9);
    assert!((rscores[1] - 0.9).abs() < 1e-9);
}
