//! AC4: A measure present in catalog A but absent in catalog B appears in only_in_a.

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
fn ac4_extra_measure_in_a_appears_in_only_in_a() {
    // catalog_only_in_a has "Exclusive Measure A" + "Total Store Sales"
    // catalog_only_in_b has only "Total Store Sales"
    let a = load("tests/fixtures/catalog_only_in_a.json");
    let b = load("tests/fixtures/catalog_only_in_b.json");

    let config = DiffConfig {
        cluster_a: "prod".into(),
        cluster_b: "staging".into(),
        numeric_tolerance: 0.001,
    };
    let report = diff_catalogs(&a, &b, &config);

    // overall should show diverge (extra entity in A)
    assert!(
        report.overall_verdict == OverallVerdict::Diverge
            || report.overall_verdict == OverallVerdict::CriticalDiverge,
        "overall_verdict should not be Agree when entity is only_in_a"
    );

    // "Exclusive Measure A" should appear in only_in_a
    let found = report
        .only_in_a
        .iter()
        .any(|e| e.unique_name == "Exclusive Measure A" && e.entity_type == "measure");
    assert!(
        found,
        "Exclusive Measure A should appear in only_in_a; got: {:?}",
        report.only_in_a
    );

    assert_eq!(
        report.summary.only_in_a, 1,
        "summary.only_in_a should be 1"
    );
    assert_eq!(
        report.summary.only_in_b, 0,
        "summary.only_in_b should be 0"
    );
}
