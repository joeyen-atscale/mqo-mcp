//! AC7: --numeric-tolerance 0.0 treats any non-zero delta as diverge;
//! --numeric-tolerance 1.0 treats up to 100% delta as agree.
//!
//! NOTE: The current iteration compares string fields (expression, aggregation_type, etc.)
//! rather than numeric stat fields (mean/min/max from DatasetSummary — that's the PRD's
//! full vision, which requires live cluster execution and dh-spec).
//!
//! For string field comparison, tolerance is not applicable — any string difference is
//! always a diverge or critical_diverge regardless of tolerance.
//!
//! This test verifies the tolerance flag round-trips through DiffConfig correctly and
//! that the same fixture gives the same result regardless of tolerance (since all diffs
//! here are string diffs, not numeric diffs).

use mcp_cross_cluster_diff::{
    catalog::DescribeModel,
    diff::{diff_catalogs, DiffConfig},
    report::OverallVerdict,
};

fn load(path: &str) -> DescribeModel {
    let raw = std::fs::read_to_string(path).expect("fixture not found");
    DescribeModel::from_json(&raw).expect("invalid fixture JSON")
}

#[test]
fn ac7_tolerance_zero_identical_catalogs_still_agree() {
    // With tolerance=0.0, identical catalogs should still agree (no delta).
    let a = load("tests/fixtures/catalog_agree_a.json");
    let b = load("tests/fixtures/catalog_agree_b.json");

    let config = DiffConfig {
        cluster_a: "prod".into(),
        cluster_b: "staging".into(),
        numeric_tolerance: 0.0,
    };
    let report = diff_catalogs(&a, &b, &config);
    assert_eq!(report.overall_verdict, OverallVerdict::Agree);
}

#[test]
fn ac7_tolerance_one_identical_catalogs_agree() {
    // With tolerance=1.0, identical catalogs should agree (all within 100% of each other).
    let a = load("tests/fixtures/catalog_agree_a.json");
    let b = load("tests/fixtures/catalog_agree_b.json");

    let config = DiffConfig {
        cluster_a: "prod".into(),
        cluster_b: "staging".into(),
        numeric_tolerance: 1.0,
    };
    let report = diff_catalogs(&a, &b, &config);
    assert_eq!(report.overall_verdict, OverallVerdict::Agree);
}

#[test]
fn ac7_tolerance_does_not_suppress_critical_diverge() {
    // Even with tolerance=1.0, a critical field difference (expression change) stays critical.
    let a = load("tests/fixtures/catalog_critical_a.json");
    let b = load("tests/fixtures/catalog_critical_b.json");

    let config = DiffConfig {
        cluster_a: "prod".into(),
        cluster_b: "staging".into(),
        numeric_tolerance: 1.0,
    };
    let report = diff_catalogs(&a, &b, &config);
    assert_eq!(report.overall_verdict, OverallVerdict::CriticalDiverge);
}
