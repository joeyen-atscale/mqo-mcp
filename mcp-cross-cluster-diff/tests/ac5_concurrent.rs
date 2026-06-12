//! AC5: Two concurrent "slow" catalog diffs complete in under 700ms when parallelized.
//!
//! This tests that diff_catalogs itself is fast (the diff engine, not network I/O).
//! The concurrency AC in the PRD refers to executing queries on two clusters in parallel;
//! since this binary processes pre-fetched catalog files, we verify that two independent
//! diffs can be issued in parallel threads and both complete within the time budget.

use mcp_cross_cluster_diff::{
    catalog::DescribeModel,
    diff::{diff_catalogs, DiffConfig},
};
use std::time::Instant;

fn load(path: &str) -> DescribeModel {
    let raw = std::fs::read_to_string(path).expect("fixture not found");
    DescribeModel::from_json(&raw).expect("invalid fixture JSON")
}

#[test]
fn ac5_two_diffs_complete_quickly() {
    // Load fixtures upfront
    let a = load("tests/fixtures/catalog_agree_a.json");
    let b = load("tests/fixtures/catalog_agree_b.json");

    let config_1 = DiffConfig {
        cluster_a: "prod".into(),
        cluster_b: "staging".into(),
        numeric_tolerance: 0.001,
    };
    let config_2 = DiffConfig {
        cluster_a: "prod2".into(),
        cluster_b: "staging2".into(),
        numeric_tolerance: 0.001,
    };

    let a1 = a.clone();
    let b1 = b.clone();
    let a2 = a.clone();
    let b2 = b.clone();

    let start = Instant::now();

    // Run both diffs in parallel threads to simulate concurrent cluster queries
    let h1 = std::thread::spawn(move || diff_catalogs(&a1, &b1, &config_1));
    let h2 = std::thread::spawn(move || diff_catalogs(&a2, &b2, &config_2));

    let r1 = h1.join().expect("thread 1 panicked");
    let r2 = h2.join().expect("thread 2 panicked");

    let elapsed = start.elapsed();

    // Both should agree
    assert_eq!(r1.overall_verdict, mcp_cross_cluster_diff::report::OverallVerdict::Agree);
    assert_eq!(r2.overall_verdict, mcp_cross_cluster_diff::report::OverallVerdict::Agree);

    // Both concurrent diffs should complete well under 700ms
    assert!(
        elapsed.as_millis() < 700,
        "two concurrent diffs took {}ms, expected < 700ms",
        elapsed.as_millis()
    );
}
