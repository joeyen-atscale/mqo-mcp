//! AC6: Malformed JSON in any input → exit 2 with a stderr diagnostic naming
//! the bad file, never a panic; a missing optional artifact degrades its ACs
//! to `skip-needs-live`, not a crash.
#![allow(clippy::expect_used, clippy::panic)]

use mcp_spike_evidence_bundle::{parse_json_file, parse_ticket_map, run_bundle, ArtifactMap};
use std::io::Write;

fn write_temp_file(content: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().expect("temp file");
    f.write_all(content.as_bytes()).expect("write temp");
    f
}

#[test]
fn ac6_malformed_artifact_json_returns_error() {
    let bad_file = write_temp_file("{ not valid json !!! }");
    let path = bad_file.path().to_str().expect("path");

    let result = parse_json_file(path);
    assert!(result.is_err(), "malformed JSON must return Err");
    let err = result.expect_err("already checked");
    // Error must name the file.
    assert!(
        err.contains(path),
        "error message should contain the file path, got: {err}"
    );
}

#[test]
fn ac6_malformed_ticket_map_returns_error() {
    let bad_file = write_temp_file("[ DEFINITELY NOT JSON }");
    let path = bad_file.path().to_str().expect("path");

    let result = parse_ticket_map(path);
    assert!(result.is_err(), "malformed ticket-map must return Err");
    let err = result.expect_err("already checked");
    assert!(
        err.contains(path),
        "error must name the bad file, got: {err}"
    );
}

#[test]
fn ac6_missing_optional_artifact_degrades_acs_not_crash() {
    // Provide ticket-map that references 'paramq' but don't provide it.
    let ticket_map_json = r#"
    {
      "tickets": [
        {
          "ticket": "TEST-001",
          "acs": [
            {
              "id": "AC1",
              "text": "needs paramq",
              "expected_artifact": "paramq",
              "verdict_if_absent": "skip-needs-live",
              "blocked_on": "live LLM vendor"
            }
          ]
        }
      ]
    }
    "#;
    let ticket_map_file = write_temp_file(ticket_map_json);
    let path = ticket_map_file.path().to_str().expect("path");

    let ticket_map = parse_ticket_map(path).expect("valid ticket-map");
    let artifacts = ArtifactMap::new(); // paramq not provided

    // Must not panic; should complete and assign skip-needs-live.
    let evidence = run_bundle(&ticket_map, &artifacts);
    assert_eq!(evidence.tickets.len(), 1);
    let ac = evidence
        .tickets
        .first()
        .and_then(|t| t.acs.first())
        .expect("first ticket's first AC must exist");
    assert_eq!(
        ac.verdict,
        mcp_spike_evidence_bundle::Verdict::SkipNeedsLive,
        "missing optional artifact must degrade to skip-needs-live"
    );
}

// Pull in tempfile as a dev-dep by using it here.
// (tempfile is used via write_temp_file helper above)
