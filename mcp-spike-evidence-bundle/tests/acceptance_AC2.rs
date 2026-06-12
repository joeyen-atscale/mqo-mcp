//! AC2: An AC whose expected artifact field is present with a real value is
//! verdicted `produced` with a `value_summary` echoing the value; an AC whose
//! artifact is absent is `skip-needs-live` or `gap` (never `produced`).
#![allow(clippy::expect_used, clippy::panic)]

use mcp_spike_evidence_bundle::{score_ac, ArtifactMap, TicketAcSpec, Verdict};
use serde_json::json;

#[test]
fn ac2_present_value_produces_produced_verdict() {
    let spec = TicketAcSpec {
        id: "AC1".to_owned(),
        text: "tool result rows".to_owned(),
        expected_artifact: "footprint".to_owned(),
        artifact_pointer: Some("/classes/tool_result_rows".to_owned()),
        owner: None,
        verdict_if_absent: None,
        blocked_on: None,
    };
    let mut artifacts = ArtifactMap::new();
    artifacts.insert(
        "footprint".to_owned(),
        json!({"classes": {"tool_result_rows": 2100}}),
    );
    let result = score_ac(&spec, &artifacts);
    assert_eq!(result.verdict, Verdict::Produced, "should be produced when value exists");
    assert!(
        result.value_summary.contains("2100"),
        "value_summary should echo the value, got: {}",
        result.value_summary
    );
}

#[test]
fn ac2_absent_artifact_never_produces_produced() {
    let spec = TicketAcSpec {
        id: "AC3".to_owned(),
        text: "live vendor loop".to_owned(),
        expected_artifact: "live_artifact_not_present".to_owned(),
        artifact_pointer: None,
        owner: None,
        verdict_if_absent: None,
        blocked_on: None,
    };
    let artifacts = ArtifactMap::new(); // empty — artifact absent
    let result = score_ac(&spec, &artifacts);
    assert_ne!(
        result.verdict,
        Verdict::Produced,
        "absent artifact must never be verdicted produced"
    );
}

#[test]
fn ac2_null_value_is_not_produced() {
    let spec = TicketAcSpec {
        id: "AC2".to_owned(),
        text: "null pointer field".to_owned(),
        expected_artifact: "footprint".to_owned(),
        artifact_pointer: Some("/missing_field".to_owned()),
        owner: None,
        verdict_if_absent: None,
        blocked_on: None,
    };
    let mut artifacts = ArtifactMap::new();
    artifacts.insert("footprint".to_owned(), json!({"other": 1}));
    let result = score_ac(&spec, &artifacts);
    assert_ne!(
        result.verdict,
        Verdict::Produced,
        "pointer not found must not be produced"
    );
}
