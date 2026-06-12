//! AC3: Catalog B has different expression and aggregation_type → critical_diverge, exit 2.

use mcp_cross_cluster_diff::{
    catalog::DescribeModel,
    diff::{diff_catalogs, exit_code, DiffConfig},
    report::{OverallVerdict, Verdict},
};

fn load(path: &str) -> DescribeModel {
    let raw = std::fs::read_to_string(path).expect("fixture not found");
    DescribeModel::from_json(&raw).expect("invalid fixture JSON")
}

#[test]
fn ac3_expression_diff_is_critical_diverge() {
    let a = load("tests/fixtures/catalog_critical_a.json");
    let b = load("tests/fixtures/catalog_critical_b.json");

    let config = DiffConfig {
        cluster_a: "prod".into(),
        cluster_b: "staging".into(),
        numeric_tolerance: 0.001,
    };
    let report = diff_catalogs(&a, &b, &config);

    assert_eq!(
        report.overall_verdict,
        OverallVerdict::CriticalDiverge,
        "overall_verdict should be CriticalDiverge"
    );
    assert_eq!(
        exit_code(&report.overall_verdict),
        2,
        "exit code should be 2 for critical_diverge"
    );

    let tss = report
        .differences
        .iter()
        .find(|d| d.unique_name == "Total Store Sales")
        .expect("Total Store Sales should be in differences");

    assert_eq!(tss.verdict, Verdict::CriticalDiverge);

    // expression field should be marked critical
    let expr_diff = tss
        .field_diffs
        .iter()
        .find(|fd| fd.field == "expression")
        .expect("expression field diff should be present");
    assert!(expr_diff.critical, "expression diff should be critical");

    // aggregation_type field should be marked critical
    let agg_diff = tss
        .field_diffs
        .iter()
        .find(|fd| fd.field == "aggregation_type")
        .expect("aggregation_type field diff should be present");
    assert!(agg_diff.critical, "aggregation_type diff should be critical");
}
