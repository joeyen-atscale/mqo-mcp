//! AC3: A `skip-needs-live` verdict carries a non-empty `blocked_on` naming
//! the unmet dependency; the run still exits 0.
#![allow(clippy::expect_used, clippy::panic)]

use mcp_spike_evidence_bundle::{score_ac, ArtifactMap, TicketAcSpec, Verdict};

#[test]
fn ac3_skip_needs_live_has_nonempty_blocked_on() {
    let spec = TicketAcSpec {
        id: "AC3".to_owned(),
        text: "live vendor compliance loop".to_owned(),
        expected_artifact: "paramq".to_owned(),
        artifact_pointer: Some("/live_vendor_score".to_owned()),
        owner: None,
        verdict_if_absent: Some("skip-needs-live".to_owned()),
        blocked_on: Some("live LLM vendor".to_owned()),
    };
    let artifacts = ArtifactMap::new(); // paramq not provided
    let result = score_ac(&spec, &artifacts);

    assert_eq!(result.verdict, Verdict::SkipNeedsLive, "should be skip-needs-live");
    let blocked = result
        .blocked_on
        .as_deref()
        .expect("blocked_on must be present for skip-needs-live");
    assert!(!blocked.is_empty(), "blocked_on must be non-empty");
    assert!(
        blocked.contains("LLM") || blocked.contains("live") || blocked.contains("vendor"),
        "blocked_on should name a live dependency, got: {blocked}"
    );
}

#[test]
fn ac3_skip_needs_live_with_artifact_present_but_field_absent() {
    // Artifact present but the pointer doesn't resolve — should still be
    // skip-needs-live because verdict_if_absent is set.
    let spec = TicketAcSpec {
        id: "AC3".to_owned(),
        text: "live loop".to_owned(),
        expected_artifact: "paramq".to_owned(),
        artifact_pointer: Some("/live_vendor_score".to_owned()),
        owner: None,
        verdict_if_absent: Some("skip-needs-live".to_owned()),
        blocked_on: Some("live LLM vendor".to_owned()),
    };
    let mut artifacts = ArtifactMap::new();
    artifacts.insert(
        "paramq".to_owned(),
        serde_json::json!({"overall": {"structured_pass_at_1": 0.8}}),
    );
    let result = score_ac(&spec, &artifacts);

    assert_eq!(result.verdict, Verdict::SkipNeedsLive);
    let blocked = result.blocked_on.as_deref().expect("blocked_on present");
    assert!(!blocked.is_empty());
}
