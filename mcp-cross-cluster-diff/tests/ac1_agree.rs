//! AC1: Two identical catalogs produce overall_verdict "agree", exit 0.

use mcp_cross_cluster_diff::{
    catalog::DescribeModel,
    diff::{diff_catalogs, exit_code, DiffConfig},
    report::OverallVerdict,
};

fn load(path: &str) -> DescribeModel {
    let raw = std::fs::read_to_string(path).expect("fixture not found");
    DescribeModel::from_json(&raw).expect("invalid fixture JSON")
}

#[test]
fn ac1_identical_catalogs_agree() {
    let a = load("tests/fixtures/catalog_agree_a.json");
    let b = load("tests/fixtures/catalog_agree_b.json");

    let config = DiffConfig {
        cluster_a: "prod".into(),
        cluster_b: "staging".into(),
        numeric_tolerance: 0.001,
    };
    let report = diff_catalogs(&a, &b, &config);

    assert_eq!(
        report.overall_verdict,
        OverallVerdict::Agree,
        "overall_verdict should be Agree"
    );
    assert_eq!(
        exit_code(&report.overall_verdict),
        0,
        "exit code should be 0 for agree"
    );

    // All measures in both A and B should agree
    for diff in &report.differences {
        assert_eq!(
            diff.verdict,
            mcp_cross_cluster_diff::report::Verdict::Agree,
            "entity {} should have verdict Agree",
            diff.unique_name
        );
    }

    assert_eq!(report.summary.diverge, 0);
    assert_eq!(report.summary.critical_diverge, 0);
    assert_eq!(report.summary.only_in_a, 0);
    assert_eq!(report.summary.only_in_b, 0);
    // 2 measures + 2 dimensions = 4 agree
    assert_eq!(report.summary.agree, 4);
}
