//! AC2: A TOML with two entries sharing the same `name` fails validation
//! with `RegistryError::DuplicateName`.

use mcp_cluster_registry::{ClusterRegistry, RegistryError};

const DUPLICATE_TOML: &str = r#"
[[clusters]]
name = "prod"
endpoint = "mcp-aws.atscaleinternal.com:15432"
supported_backends = ["sql"]
priority = 0
required = true

[clusters.auth]
type = "direct"
pg_user = "atscale_user"
pg_pass_env = "PROD_PG_PASS"

[[clusters]]
name = "prod"
endpoint = "mcp-aws2.atscaleinternal.com:15432"
supported_backends = ["sql"]
priority = 1
required = false

[clusters.auth]
type = "direct"
pg_user = "atscale_user2"
pg_pass_env = "PROD_PG_PASS2"
"#;

#[test]
fn duplicate_name_fails_validation() {
    let reg = ClusterRegistry::from_toml(DUPLICATE_TOML).expect("TOML itself parses fine");
    let errs = reg.validate().expect_err("validation must fail");
    let has_duplicate = errs
        .iter()
        .any(|e| matches!(e, RegistryError::DuplicateName(n) if n == "prod"));
    assert!(has_duplicate, "expected DuplicateName(\"prod\"), got: {errs:?}");
}
