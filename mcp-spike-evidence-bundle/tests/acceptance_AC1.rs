//! AC1: Given all four artifact inputs + a ticket-map, the bundle emits
//! `spike_evidence.json` with one entry per ticket and a per-AC verdict for
//! every AC in the map.
#![allow(clippy::expect_used, clippy::panic, clippy::uninlined_format_args)]

use mcp_spike_evidence_bundle::{parse_json_file, parse_ticket_map, run_bundle, ArtifactMap};

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

#[test]
fn ac1_all_inputs_produce_per_ticket_per_ac_entries() {
    let fixtures = fixtures_dir();

    let ticket_map = parse_ticket_map(fixtures.join("ticket_map.json").to_str().expect("valid path"))
        .expect("ticket_map.json must parse");

    let mut artifacts = ArtifactMap::new();
    for (key, name) in &[
        ("footprint", "footprint.json"),
        ("paramq", "bench_report.json"),
        ("walkthrough", "walkthrough.json"),
        ("handle_demo", "handle_demo.json"),
    ] {
        let val = parse_json_file(fixtures.join(name).to_str().expect("valid path"))
            .expect("artifact must parse");
        artifacts.insert((*key).to_owned(), val);
    }

    let evidence = run_bundle(&ticket_map, &artifacts);

    // One entry per ticket.
    assert_eq!(evidence.tickets.len(), 4, "expected 4 ticket entries");

    // Every ticket has the expected number of ACs from the map.
    let expected_ac_counts = [
        ("ATSCALE-49212", 3usize),
        ("ATSCALE-49213", 3),
        ("ATSCALE-49214", 2),
        ("ATSCALE-49215", 2),
    ];

    for (expected_ticket, expected_count) in &expected_ac_counts {
        let ticket_result = evidence
            .tickets
            .iter()
            .find(|t| t.ticket == *expected_ticket)
            .unwrap_or_else(|| panic!("ticket {} missing from output", expected_ticket));

        assert_eq!(
            ticket_result.acs.len(),
            *expected_count,
            "ticket {} should have {} ACs, got {}",
            expected_ticket,
            expected_count,
            ticket_result.acs.len()
        );
    }
}
