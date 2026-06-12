//! Shared test helpers: ephemeral TCP listeners and registry TOML builder.
#![allow(dead_code)]

use tempfile::NamedTempFile;
use tokio::net::TcpListener;

/// Bind a TcpListener on 127.0.0.1:0 (OS-assigned port) and return it.
/// Keep it alive for the duration of the test; accepting connections is not required
/// for the TCP-connect probe to succeed — the OS completes the SYN/SYN-ACK before
/// the listener's accept() call.
pub async fn bind_free_port() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

/// Return a port that is NOT listening (bind and immediately drop).
pub async fn refused_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    // Brief yield to let the OS reclaim the port
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    port
}

/// Build a minimal TOML registry string with a single cluster.
pub fn single_cluster_toml(name: &str, host_port: &str, required: bool) -> String {
    format!(
        r#"[[clusters]]
name = "{name}"
endpoint = "{host_port}"
required = {required}
priority = 1
supported_backends = ["sql"]

[clusters.auth]
type = "direct"
pg_user = "PG_USER"
pg_pass_env = "PG_PASS"
"#
    )
}

/// Build a multi-cluster TOML registry string.
pub fn multi_cluster_toml(entries: &[(&str, &str, bool)]) -> String {
    let mut s = String::new();
    for (name, host_port, required) in entries {
        s.push_str(&single_cluster_toml(name, host_port, *required));
        s.push('\n');
    }
    s
}

/// Write TOML content to a named temp file and return it (keep alive).
pub fn write_temp_toml(content: &str) -> NamedTempFile {
    let f = NamedTempFile::new().unwrap();
    std::fs::write(f.path(), content).unwrap();
    f
}

/// Parse a registry from TOML and probe all clusters.
pub async fn run_probe(
    toml: &str,
    timeout_ms: u64,
) -> mcp_cluster_health_monitor::report::HealthReport {
    let registry = mcp_cluster_registry::ClusterRegistry::from_toml(toml).unwrap();
    let refs: Vec<&mcp_cluster_registry::ClusterEntry> = registry.clusters.iter().collect();
    let health = mcp_cluster_health_monitor::probe::probe_clusters(&refs, timeout_ms).await;
    mcp_cluster_health_monitor::report::HealthReport::from_health(&health)
}
