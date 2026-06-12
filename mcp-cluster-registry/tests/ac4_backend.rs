//! AC4: `supports_backend` returns the correct bool based on each cluster's
//! `supported_backends` list.

use mcp_cluster_registry::ClusterRegistry;

const TOML: &str = r#"
[[clusters]]
name = "sql-only"
endpoint = "sql.example.com:15432"
supported_backends = ["sql"]
priority = 0
required = true

[clusters.auth]
type = "direct"
pg_user = "u"
pg_pass_env = "P"

[[clusters]]
name = "full"
endpoint = "full.example.com:15432"
supported_backends = ["sql", "dax", "mdx"]
priority = 1
required = false

[clusters.auth]
type = "direct"
pg_user = "u"
pg_pass_env = "P"
"#;

#[test]
fn sql_only_cluster_does_not_support_dax() {
    let reg = ClusterRegistry::from_toml(TOML).unwrap();
    assert!(!reg.supports_backend("sql-only", "dax"));
}

#[test]
fn sql_only_cluster_supports_sql() {
    let reg = ClusterRegistry::from_toml(TOML).unwrap();
    assert!(reg.supports_backend("sql-only", "sql"));
}

#[test]
fn full_cluster_supports_dax() {
    let reg = ClusterRegistry::from_toml(TOML).unwrap();
    assert!(reg.supports_backend("full", "dax"));
}

#[test]
fn full_cluster_supports_mdx() {
    let reg = ClusterRegistry::from_toml(TOML).unwrap();
    assert!(reg.supports_backend("full", "mdx"));
}

#[test]
fn unknown_cluster_returns_false() {
    let reg = ClusterRegistry::from_toml(TOML).unwrap();
    assert!(!reg.supports_backend("nonexistent", "sql"));
}
