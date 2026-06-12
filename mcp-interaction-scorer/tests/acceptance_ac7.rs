//! AC7 (MAY): BufRead trait variant produces same output as file-path variant.

use mcp_interaction_scorer::{score_reader, score_trace_store};
use mcp_trace_store::{BindOutcome, ExecuteOutcome, QualitySignals, TraceRecord};
use serde_json::json;
use std::io::Write;

fn make_record(session_id: &str) -> TraceRecord {
    TraceRecord {
        record_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_owned(),
        cluster_name: None,
        timestamp_ms: 1_000,
        mqo: json!({"measures": [{"unique_name": "revenue"}]}),
        bind_outcome: BindOutcome::Success,
        grounding_score: Some(0.8),
        grounding_band: None,
        execute_result: ExecuteOutcome::Success { row_count: 3, result_empty: false },
        quality: QualitySignals {
            first_attempt_bind: true,
            bind_attempt_count: 1,
            total_latency_ms: 50,
            tokens_used: None,
        },
        user_question: None,
    }
}

#[test]
fn ac7_bufread_variant_matches_file_path_variant() {
    let records: Vec<TraceRecord> = vec![
        make_record("alpha"),
        make_record("beta"),
    ];

    let jsonl: Vec<u8> = records
        .iter()
        .flat_map(|r| {
            let mut s = serde_json::to_string(r).unwrap();
            s.push('\n');
            s.into_bytes()
        })
        .collect();

    // Write to a temp file.
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(&jsonl).unwrap();
    tmp.flush().unwrap();

    let file_report = score_trace_store(tmp.path()).expect("file-path variant failed");
    let reader_report =
        score_reader(std::io::Cursor::new(&jsonl)).expect("BufRead variant failed");

    // Both should have the same sessions and entities.
    assert_eq!(
        file_report.sessions.len(),
        reader_report.sessions.len(),
        "session count mismatch"
    );
    assert_eq!(
        file_report.entities.len(),
        reader_report.entities.len(),
        "entity count mismatch"
    );

    for (sid, fq) in &file_report.sessions {
        let rq = reader_report
            .sessions
            .get(sid)
            .unwrap_or_else(|| panic!("session {sid} missing from reader report"));
        assert!((fq.retry_rate - rq.retry_rate).abs() < 1e-9);
        assert!((fq.empty_result_rate - rq.empty_result_rate).abs() < 1e-9);
        assert!((fq.bind_failure_rate - rq.bind_failure_rate).abs() < 1e-9);
        assert_eq!(fq.total_mqos, rq.total_mqos);
    }
}
