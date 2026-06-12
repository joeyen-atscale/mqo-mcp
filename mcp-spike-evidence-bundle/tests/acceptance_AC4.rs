//! AC4: The per-ticket summary counts sum to the number of ACs in the map for
//! that ticket (every AC gets exactly one verdict — no AC dropped, no
//! double-count).
#![allow(clippy::expect_used, clippy::panic)]

use mcp_spike_evidence_bundle::{parse_json_file, parse_ticket_map, run_bundle, ArtifactMap};

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

#[test]
fn ac4_summary_counts_sum_to_ac_count() {
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

    for ticket_result in &evidence.tickets {
        let s = &ticket_result.summary;
        let total_from_summary = s.produced + s.gap + s.skip_needs_live;
        let ac_count = ticket_result.acs.len();

        assert_eq!(
            total_from_summary,
            ac_count,
            "ticket {} summary counts ({produced}+{gap}+{skip}) = {total} but ac_count = {ac_count}",
            ticket_result.ticket,
            produced = s.produced,
            gap = s.gap,
            skip = s.skip_needs_live,
            total = total_from_summary,
            ac_count = ac_count,
        );
    }
}

#[test]
fn ac4_no_ac_dropped_every_id_appears_once() {
    let fixtures = fixtures_dir();

    let ticket_map = parse_ticket_map(fixtures.join("ticket_map.json").to_str().expect("valid path"))
        .expect("ticket_map.json must parse");
    let artifacts = ArtifactMap::new(); // empty — all gap/skip

    let evidence = run_bundle(&ticket_map, &artifacts);

    for (ticket_spec, ticket_result) in
        ticket_map.tickets.iter().zip(evidence.tickets.iter())
    {
        let expected_ids: Vec<&str> = ticket_spec.acs.iter().map(|a| a.id.as_str()).collect();
        let actual_ids: Vec<&str> = ticket_result.acs.iter().map(|a| a.id.as_str()).collect();

        assert_eq!(
            expected_ids, actual_ids,
            "ticket {} AC IDs in output must match input map order",
            ticket_spec.ticket
        );
    }
}
