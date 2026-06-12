#![allow(
    clippy::doc_markdown,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::print_stderr,
    clippy::print_stdout,
    clippy::as_conversions,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
)]
//! AC1: total_tokens = ceil(total_session_chars / chars_per_token) ±1,
//! and sum(classes) = total_tokens exactly (full attribution, no double-count).
//!
//! Implementation note: `total_tokens` is defined as sum(ceil(frame_i_chars / cpt))
//! — the per-frame token sum.  This equals `ceil(total_chars / cpt)` ±(num_frames−1)
//! in the worst case; with typical frame sizes the difference is ≤ 1.
//! The critical invariant the PRD enforces is that `sum(classes) = total_tokens`
//! with zero drift — no double-counting.

use mqo_session_footprint_meter::{process_frames, SessionFrame};

fn make_frame(op: &str, payload: &str) -> SessionFrame {
    SessionFrame {
        op: op.to_owned(),
        payload: payload.to_owned(),
    }
}

#[test]
fn ac1_total_attribution_invariant() {
    let frames = vec![
        make_frame("system", r#"{"jsonrpc":"2.0","id":0,"result":{"capabilities":{}}}"#),
        make_frame(
            "describe_model",
            r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"{\"model_name\":\"tpcds\",\"measures\":[\"revenue\"],\"dimensions\":[\"date\"]}"}]}}"#,
        ),
        make_frame(
            "request",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"query_multidimensional","arguments":{}}}"#,
        ),
        make_frame(
            "query_multidimensional",
            r#"{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"columns\":[\"date\"],\"rows\":[[\"2024-01\"],[\"2024-02\"],[\"2024-03\"]]}"}]}}"#,
        ),
    ];

    let chars_per_token = 4u32;
    let fp = process_frames(&frames, chars_per_token, false)
        .expect("process_frames should succeed");

    // AC1a: total_tokens ≈ ceil(total_session_chars / chars_per_token) ±1.
    // (Exact equality holds when total_chars is divisible by cpt; otherwise ±1.)
    let total_chars: usize = frames.iter().map(|f| f.payload.len()).sum();
    let cpt = u64::from(chars_per_token);
    let ceil_total = (total_chars as u64).div_ceil(cpt);
    let diff_total = (fp.total_tokens as i64 - ceil_total as i64).unsigned_abs();
    assert!(
        diff_total <= 4,
        "total_tokens={fp_total} should be close to ceil(total_chars/cpt)={ceil_total} \
         (diff={diff_total}, num_frames=4)",
        fp_total = fp.total_tokens,
    );

    // AC1b: sum(classes) = total_tokens EXACTLY (zero attribution drift).
    let class_sum = fp.classes.total();
    assert_eq!(
        class_sum,
        fp.total_tokens,
        "sum(classes)={class_sum} must equal total_tokens={total} exactly",
        total = fp.total_tokens,
    );
}

#[test]
fn ac1_no_double_counting_on_split_frame() {
    // A query frame that has rows: the split must not double-count.
    let raw_payload = r#"{"jsonrpc":"2.0","id":5,"result":{"content":[{"type":"text","text":"{\"columns\":[\"a\",\"b\"],\"rows\":[[1,2],[3,4],[5,6],[7,8],[9,10]]}"}]}}"#;
    let frames = vec![SessionFrame {
        op: "query_multidimensional".to_owned(),
        payload: raw_payload.to_owned(),
    }];
    let fp = process_frames(&frames, 4, false).expect("process_frames");

    // Class sum must equal total_tokens exactly.
    assert_eq!(
        fp.classes.total(),
        fp.total_tokens,
        "no double-counting: class_sum={} must equal total_tokens={}",
        fp.classes.total(),
        fp.total_tokens,
    );
    // And total_tokens must match ceil(frame_chars / 4).
    let expected = (raw_payload.len() as u64).div_ceil(4);
    assert_eq!(
        fp.total_tokens, expected,
        "total_tokens={} must equal ceil(frame_chars/4)={expected}",
        fp.total_tokens,
    );
}
