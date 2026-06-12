//! AC5: `to_json` round-trips: TOML → ClusterRegistry → JSON → ClusterRegistry
//! produces an equivalent registry.

use mcp_cluster_registry::ClusterRegistry;

const TOML: &str = r#"
[[clusters]]
name = "prod"
endpoint = "mcp-aws.atscaleinternal.com:15432"
supported_backends = ["sql"]
priority = 0
required = true
tags = ["prod"]

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
fn toml_to_json_to_registry_round_trips() {
    let original = ClusterRegistry::from_toml(TOML).expect("TOML parse");
    let json = original.to_json();
    let restored = ClusterRegistry::from_json(&json).expect("JSON parse");
    assert_eq!(original, restored);
}

#[test]
fn round_trip_preserves_cluster_count() {
    let original = ClusterRegistry::from_toml(TOML).unwrap();
    let json = original.to_json();
    let restored = ClusterRegistry::from_json(&json).unwrap();
    assert_eq!(original.clusters.len(), restored.clusters.len());
}

#[test]
fn round_trip_json_is_valid_json() {
    let reg = ClusterRegistry::from_toml(TOML).unwrap();
    let json = reg.to_json();
    // Must parse as a JSON value without error.
    let _: serde_json::Value = serde_json::from_str(&json).expect("to_json must emit valid JSON");
}
