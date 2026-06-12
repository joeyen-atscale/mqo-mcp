//! AC6: Registry parse failure exits 2 with a diagnostic; no cluster probes attempted.
//!
//! We test directly via the library API: from_toml() on bad TOML must return an error.

#[test]
fn ac6_bad_toml_returns_error() {
    let bad_toml = "this is not valid toml ][[[";
    let result = mcp_cluster_registry::ClusterRegistry::from_toml(bad_toml);
    assert!(
        result.is_err(),
        "bad TOML should produce a parse error"
    );
    let err = result.unwrap_err();
    let err_str = err.to_string();
    assert!(
        !err_str.is_empty(),
        "error message should be non-empty: got {:?}",
        err_str
    );
}

#[test]
fn ac6_empty_toml_returns_error_or_empty_clusters() {
    // An empty TOML file produces no clusters — validate() should catch it
    let empty_toml = "";
    let result = mcp_cluster_registry::ClusterRegistry::from_toml(empty_toml);
    // Either parse error OR empty clusters
    match result {
        Err(_) => {} // parse error — ok
        Ok(r) => {
            // Parsed successfully but empty — validate() should catch it
            let v = r.validate();
            assert!(v.is_err(), "empty registry should fail validate()");
        }
    }
}

#[test]
fn ac6_bad_json_returns_error() {
    let bad_json = "{not json at all";
    let result = mcp_cluster_registry::ClusterRegistry::from_json(bad_json);
    assert!(result.is_err(), "bad JSON should produce a parse error");
}
