//! AC2: Catalog B has a different folder for Total Store Sales → verdict "diverge", exit 1.

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
fn ac2_non_critical_field_diff_is_diverge() {
    let a = load("tests/fixtures/catalog_diverge_a.json");
    let b = load("tests/fixtures/catalog_diverge_b.json");

    let config = DiffConfig {
        cluster_a: "prod".into(),
        cluster_b: "staging".into(),
        numeric_tolerance: 0.001,
    };
    let report = diff_catalogs(&a, &b, &config);

    assert_eq!(
        report.overall_verdict,
        OverallVerdict::Diverge,
        "overall_verdict should be Diverge"
    );
    assert_eq!(
        exit_code(&report.overall_verdict),
        1,
        "exit code should be 1 for diverge"
    );

    // Total Store Sales should have verdict Diverge
    let tss = report
        .differences
        .iter()
        .find(|d| d.unique_name == "Total Store Sales")
        .expect("Total Store Sales should be in differences");

    assert_eq!(tss.verdict, Verdict::Diverge);
    // folder field should be the differing field
    assert!(
        tss.field_diffs.iter().any(|fd| fd.field == "folder"),
        "folder field should appear in field_diffs"
    );
    // folder diff should not be critical
    let folder_diff = tss.field_diffs.iter().find(|fd| fd.field == "folder").unwrap();
    assert!(!folder_diff.critical, "folder diff should not be critical");
}
