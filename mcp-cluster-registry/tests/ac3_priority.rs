//! AC3: `by_priority` returns clusters sorted ascending by `priority` (0 first).

use mcp_cluster_registry::ClusterRegistry;

const TOML: &str = r#"
[[clusters]]
name = "low"
endpoint = "low.example.com:15432"
supported_backends = ["sql"]
priority = 200
required = false

[clusters.auth]
type = "direct"
pg_user = "u"
pg_pass_env = "P"

[[clusters]]
name = "high"
endpoint = "high.example.com:15432"
supported_backends = ["sql"]
priority = 0
required = true

[clusters.auth]
type = "direct"
pg_user = "u"
pg_pass_env = "P"

[[clusters]]
name = "mid"
endpoint = "mid.example.com:15432"
supported_backends = ["sql"]
priority = 100
required = false

[clusters.auth]
type = "direct"
pg_user = "u"
pg_pass_env = "P"
"#;

#[test]
fn by_priority_ascending() {
    let reg = ClusterRegistry::from_toml(TOML).unwrap();
    let ordered = reg.by_priority();
    assert_eq!(ordered.len(), 3);
    assert_eq!(ordered[0].name, "high");   // priority = 0
    assert_eq!(ordered[1].name, "mid");    // priority = 100
    assert_eq!(ordered[2].name, "low");    // priority = 200
}

#[test]
fn priorities_are_non_decreasing() {
    let reg = ClusterRegistry::from_toml(TOML).unwrap();
    let ordered = reg.by_priority();
    let priorities: Vec<u8> = ordered.iter().map(|c| c.priority).collect();
    let mut sorted = priorities.clone();
    sorted.sort();
    assert_eq!(priorities, sorted);
}
