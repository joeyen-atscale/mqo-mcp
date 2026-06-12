//! AC5: `--format markdown` emits one section per ticket with an AC→verdict→
//! evidence table, and `skip-needs-live` rows render their `blocked_on`.
#![allow(clippy::expect_used, clippy::panic)]

use mcp_spike_evidence_bundle::{
    parse_json_file, parse_ticket_map, render_markdown, run_bundle, ArtifactMap,
};

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

#[test]
fn ac5_markdown_has_one_section_per_ticket() {
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
    let md = render_markdown(&evidence);

    // One H2 section per ticket.
    for ticket in &["ATSCALE-49212", "ATSCALE-49213", "ATSCALE-49214", "ATSCALE-49215"] {
        assert!(
            md.contains(&format!("## {ticket}")),
            "markdown should contain section for {ticket}"
        );
    }
}

#[test]
fn ac5_markdown_skip_needs_live_renders_blocked_on() {
    let fixtures = fixtures_dir();

    let ticket_map = parse_ticket_map(fixtures.join("ticket_map.json").to_str().expect("valid path"))
        .expect("ticket_map.json must parse");

    // Provide only footprint — paramq absent so 49213 AC3 (skip-needs-live) fires.
    let mut artifacts = ArtifactMap::new();
    let footprint_val = parse_json_file(
        fixtures.join("footprint.json").to_str().expect("valid path"),
    )
    .expect("footprint must parse");
    artifacts.insert("footprint".to_owned(), footprint_val);

    let evidence = run_bundle(&ticket_map, &artifacts);
    let md = render_markdown(&evidence);

    // The 49213 AC3 skip-needs-live row must render the blocked_on dependency.
    assert!(
        md.contains("live LLM vendor"),
        "markdown should contain 'live LLM vendor' for 49213 AC3 skip row, got:\n{md}"
    );
    assert!(
        md.contains("skip-needs-live"),
        "markdown should contain 'skip-needs-live' verdict"
    );
}

#[test]
fn ac5_markdown_contains_ac_table_header() {
    let fixtures = fixtures_dir();
    let ticket_map = parse_ticket_map(fixtures.join("ticket_map.json").to_str().expect("valid path"))
        .expect("ticket_map.json must parse");
    let artifacts = ArtifactMap::new();
    let evidence = run_bundle(&ticket_map, &artifacts);
    let md = render_markdown(&evidence);

    assert!(
        md.contains("| AC | Verdict |"),
        "markdown should contain an AC table header"
    );
}
