//! AC1: A valid TOML string with 2 cluster entries parses to a ClusterRegistry
//! with 2 entries, both field-correct.

use mcp_cluster_registry::{AuthConfig, ClusterRegistry};

const TOML: &str = r#"
[[clusters]]
name = "prod"
endpoint = "mcp-aws.atscaleinternal.com:15432"
supported_backends = ["sql"]
priority = 0
required = true
tags = ["prod", "snowflake"]

[clusters.auth]
type = "direct"
pg_user = "atscale_user"
pg_pass_env = "PROD_PG_PASS"

[[clusters]]
name = "staging"
endpoint = "mcp-staging.atscaleinternal.com:15432"
xmla_url = "http://mcp-staging.atscaleinternal.com:11111"
supported_backends = ["sql", "dax", "mdx"]
priority = 1
required = false
model_filter = ["tpcds_benchmark_model"]

[clusters.auth]
type = "oidc"
token_url = "https://mcp-staging.atscaleinternal.com/auth/realms/AtScale/protocol/openid-connect/token"
client_id = "atscale-mcp"
realm = "AtScale"
client_secret_env = "STAGING_OIDC_SECRET"
"#;

#[test]
fn parses_two_clusters() {
    let reg = ClusterRegistry::from_toml(TOML).expect("TOML should parse");
    assert_eq!(reg.clusters.len(), 2);
}

#[test]
fn prod_cluster_fields_correct() {
    let reg = ClusterRegistry::from_toml(TOML).unwrap();
    let prod = reg.get("prod").expect("prod cluster must exist");

    assert_eq!(prod.name, "prod");
    assert_eq!(prod.endpoint, "mcp-aws.atscaleinternal.com:15432");
    assert_eq!(prod.supported_backends, vec!["sql"]);
    assert_eq!(prod.priority, 0);
    assert!(prod.required);
    assert_eq!(prod.tags, vec!["prod", "snowflake"]);
    assert!(prod.xmla_url.is_none());
    assert!(prod.model_filter.is_none());

    assert_eq!(
        prod.auth,
        AuthConfig::Direct {
            pg_user: "atscale_user".to_string(),
            pg_pass_env: "PROD_PG_PASS".to_string(),
        }
    );
}

#[test]
fn staging_cluster_fields_correct() {
    let reg = ClusterRegistry::from_toml(TOML).unwrap();
    let staging = reg.get("staging").expect("staging cluster must exist");

    assert_eq!(staging.name, "staging");
    assert_eq!(staging.endpoint, "mcp-staging.atscaleinternal.com:15432");
    assert_eq!(
        staging.xmla_url.as_deref(),
        Some("http://mcp-staging.atscaleinternal.com:11111")
    );
    assert_eq!(staging.supported_backends, vec!["sql", "dax", "mdx"]);
    assert_eq!(staging.priority, 1);
    assert!(!staging.required);
    assert_eq!(
        staging.model_filter.as_deref(),
        Some(vec!["tpcds_benchmark_model".to_string()].as_slice())
    );

    assert_eq!(
        staging.auth,
        AuthConfig::Oidc {
            token_url: "https://mcp-staging.atscaleinternal.com/auth/realms/AtScale/protocol/openid-connect/token".to_string(),
            client_id: "atscale-mcp".to_string(),
            realm: "AtScale".to_string(),
            client_secret_env: "STAGING_OIDC_SECRET".to_string(),
        }
    );
}
