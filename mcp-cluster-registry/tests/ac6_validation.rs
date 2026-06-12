//! AC6: An empty `clusters` array fails validation with `RegistryError::EmptyClusters`.
//! A cluster entry with an empty `endpoint` fails with `RegistryError::MissingRequiredField`.

use mcp_cluster_registry::{ClusterRegistry, RegistryError};

#[test]
fn empty_clusters_fails_with_empty_error() {
    let json = r#"{"clusters":[]}"#;
    let reg = ClusterRegistry::from_json(json).expect("JSON parses");
    let errs = reg.validate().expect_err("empty clusters must fail validation");
    assert!(
        errs.iter().any(|e| matches!(e, RegistryError::EmptyClusters)),
        "expected EmptyClusters, got: {errs:?}"
    );
}

#[test]
fn missing_endpoint_fails_with_missing_required_field() {
    // Build a cluster with empty endpoint via JSON so we can bypass Rust's type system.
    let json = r#"{
        "clusters": [{
            "name": "bad",
            "endpoint": "",
            "xmla_url": null,
            "auth": {"type": "direct", "pg_user": "u", "pg_pass_env": "P"},
            "supported_backends": ["sql"],
            "model_filter": null,
            "priority": 0,
            "required": true,
            "tags": []
        }]
    }"#;
    let reg = ClusterRegistry::from_json(json).expect("JSON parses");
    let errs = reg.validate().expect_err("missing endpoint must fail");
    let has_missing = errs.iter().any(|e| {
        matches!(e, RegistryError::MissingRequiredField { name, field }
            if name == "bad" && field == "endpoint")
    });
    assert!(has_missing, "expected MissingRequiredField for endpoint, got: {errs:?}");
}

#[test]
fn valid_registry_passes_validation() {
    let json = r#"{
        "clusters": [{
            "name": "ok",
            "endpoint": "ok.example.com:15432",
            "xmla_url": null,
            "auth": {"type": "direct", "pg_user": "u", "pg_pass_env": "P"},
            "supported_backends": ["sql"],
            "model_filter": null,
            "priority": 0,
            "required": true,
            "tags": []
        }]
    }"#;
    let reg = ClusterRegistry::from_json(json).expect("JSON parses");
    reg.validate().expect("valid registry must pass validation");
}
