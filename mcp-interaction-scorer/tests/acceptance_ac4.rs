//! AC4: malformed line at position 3 returns ParseError carrying line_number == 3.
//!
//! Valid lines before the error are NOT returned (entire call returns Err).

use mcp_interaction_scorer::{score_reader, ScorerError};
use mcp_trace_store::{BindOutcome, ExecuteOutcome, QualitySignals, TraceRecord};
use serde_json::json;

fn valid_record(session_id: &str) -> String {
    let r = TraceRecord {
        record_id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_owned(),
        cluster_name: None,
        timestamp_ms: 1_000,
        mqo: json!({"measures": [{"unique_name": "revenue"}]}),
        bind_outcome: BindOutcome::Success,
        grounding_score: None,
        grounding_band: None,
        execute_result: ExecuteOutcome::Success { row_count: 1, result_empty: false },
        quality: QualitySignals {
            first_attempt_bind: true,
            bind_attempt_count: 1,
            total_latency_ms: 50,
            tokens_used: None,
        },
        user_question: None,
    };
    serde_json::to_string(&r).unwrap()
}

#[test]
fn ac4_bad_line_3_returns_parse_error_with_correct_line_number() {
    // Lines 1 and 2 are valid TraceRecord JSON.
    // Line 3 is invalid JSON.
    // Lines 4+ would be valid but should never be reached.
    let line1 = valid_record("s1");
    let line2 = valid_record("s2");
    let line3 = r#"{"this": "is not a TraceRecord", "missing_required_fields": true}"#;
    let line4 = valid_record("s4");

    let jsonl = format!("{line1}\n{line2}\n{line3}\n{line4}\n");

    let result = score_reader(std::io::Cursor::new(jsonl.as_bytes()));

    match result {
        Err(ScorerError::ParseError { line_number, .. }) => {
            assert_eq!(
                line_number, 3,
                "Expected error at line 3, got line {line_number}"
            );
        }
        Err(other) => panic!("Expected ParseError, got {other:?}"),
        Ok(_) => panic!("Expected Err, got Ok — malformed line should have caused an error"),
    }
}
