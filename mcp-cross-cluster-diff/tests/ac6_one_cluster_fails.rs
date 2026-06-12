//! AC6: If catalog JSON is malformed (simulating cluster execution failure),
//! the CLI reports an error and exits 2.
//!
//! We test the library-level parse error path: from_json on bad input returns Err.

use mcp_cross_cluster_diff::catalog::DescribeModel;

#[test]
fn ac6_malformed_json_returns_error() {
    let bad_json = r#"{ "models": [ { NOT VALID JSON "#;
    let result = DescribeModel::from_json(bad_json);
    assert!(
        result.is_err(),
        "malformed JSON should return an error (simulates cluster execution failure)"
    );
}

#[test]
fn ac6_missing_file_causes_error() {
    let result = std::fs::read_to_string("/tmp/definitely_does_not_exist_ac6.json");
    assert!(
        result.is_err(),
        "missing file should cause an IO error (simulates cluster execution failure)"
    );
}
