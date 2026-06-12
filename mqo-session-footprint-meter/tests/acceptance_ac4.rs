#![allow(clippy::doc_markdown, clippy::expect_used, clippy::unwrap_used, clippy::print_stderr, clippy::print_stdout)]
//! AC4: --chars-per-token 4 on the same fixture twice is byte-identical
//! (deterministic); changing to 3 raises every class count.

use mqo_session_footprint_meter::{process_frames, SessionFrame};

fn fixture() -> Vec<SessionFrame> {
    vec![
        SessionFrame {
            op: "system".to_owned(),
            payload: r#"{"jsonrpc":"2.0","id":0,"result":{"capabilities":{}}}"#.to_owned(),
        },
        SessionFrame {
            op: "describe_model".to_owned(),
            payload: r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"{\"model_name\":\"tpcds\",\"measures\":[\"revenue\"],\"dimensions\":[\"date\"]}"}]}}"#.to_owned(),
        },
        SessionFrame {
            op: "query_multidimensional".to_owned(),
            payload: r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"columns\":[\"date\"],\"rows\":[[\"2024-01\"],[\"2024-02\"]]}"}]}}"#.to_owned(),
        },
    ]
}

#[test]
fn ac4_deterministic_same_cpt() {
    let frames = fixture();
    let fp1 = process_frames(&frames, 4, false).expect("first run");
    let fp2 = process_frames(&frames, 4, false).expect("second run");

    let json1 = serde_json::to_string(&fp1).expect("serialize fp1");
    let json2 = serde_json::to_string(&fp2).expect("serialize fp2");

    assert_eq!(json1, json2, "two runs at cpt=4 should be byte-identical");
}

#[test]
fn ac4_lower_cpt_raises_counts() {
    let frames = fixture();
    let fp4 = process_frames(&frames, 4, false).expect("cpt=4");
    let fp3 = process_frames(&frames, 3, false).expect("cpt=3");

    assert!(
        fp3.total_tokens > fp4.total_tokens,
        "cpt=3 total_tokens ({}) should be > cpt=4 ({})",
        fp3.total_tokens,
        fp4.total_tokens,
    );
    // Every class should be >= its cpt=4 counterpart.
    assert!(
        fp3.classes.system_prompt >= fp4.classes.system_prompt,
        "system_prompt: cpt3={} cpt4={}",
        fp3.classes.system_prompt,
        fp4.classes.system_prompt,
    );
    assert!(
        fp3.classes.catalog_describe_model >= fp4.classes.catalog_describe_model,
        "catalog_describe_model: cpt3={} cpt4={}",
        fp3.classes.catalog_describe_model,
        fp4.classes.catalog_describe_model,
    );
    assert!(
        fp3.classes.tool_result_rows >= fp4.classes.tool_result_rows,
        "tool_result_rows: cpt3={} cpt4={}",
        fp3.classes.tool_result_rows,
        fp4.classes.tool_result_rows,
    );
}
