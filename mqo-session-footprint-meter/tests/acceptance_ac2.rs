#![allow(clippy::doc_markdown, clippy::expect_used, clippy::unwrap_used, clippy::print_stderr, clippy::print_stdout)]
//! AC2: A query_multidimensional response is split so its `rows` array is
//! attributed to tool_result_rows and its envelope to tool_call.
//! A fixture with a known 5000-row result reports tool_result_rows > tool_call.

use mqo_session_footprint_meter::{process_frames, SessionFrame};

fn big_query_frame() -> SessionFrame {
    // Build a payload with many rows so tool_result_rows >> tool_call.
    let rows: Vec<String> = (0..5000)
        .map(|i| format!("[\"row_{i}\",{i},\"some-value-{i}\"]"))
        .collect();
    let rows_json = rows.join(",");
    let inner = format!(
        r#"{{"columns":["name","id","val"],"rows":[{rows_json}]}}"#
    );
    // Escape for embedding in the MCP content wrapper.
    let escaped = inner.replace('\\', "\\\\").replace('"', "\\\"");
    let payload = format!(
        r#"{{"jsonrpc":"2.0","id":10,"result":{{"content":[{{"type":"text","text":"{escaped}"}}]}}}}"#
    );
    SessionFrame {
        op: "query_multidimensional".to_owned(),
        payload,
    }
}

#[test]
fn ac2_rows_greater_than_envelope() {
    let frames = vec![big_query_frame()];
    let fp = process_frames(&frames, 4, false).expect("process_frames should succeed");

    assert!(
        fp.classes.tool_result_rows > fp.classes.tool_call,
        "tool_result_rows ({}) should be > tool_call ({})",
        fp.classes.tool_result_rows,
        fp.classes.tool_call,
    );
    // Both should be non-zero.
    assert!(fp.classes.tool_result_rows > 0, "tool_result_rows should be > 0");
    assert!(fp.classes.tool_call > 0, "tool_call should be > 0");
}
