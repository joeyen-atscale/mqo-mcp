//! AC3: nonexistent path returns Err with IoError variant; no panic.

use mcp_interaction_scorer::{score_trace_store, ScorerError};

#[test]
fn ac3_nonexistent_path_returns_io_error() {
    let result = std::panic::catch_unwind(|| {
        score_trace_store("/nonexistent/path/that/cannot/possibly/exist/trace.jsonl")
    });

    // Must not panic.
    let scored = result.expect("score_trace_store panicked on nonexistent path");

    // Must be an Err of variant IoError.
    match scored {
        Err(ScorerError::IoError(_)) => {}
        Err(other) => panic!("Expected IoError, got: {other:?}"),
        Ok(_) => panic!("Expected Err, got Ok"),
    }
}
