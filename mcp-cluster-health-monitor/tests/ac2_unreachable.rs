//! AC2: A cluster whose TCP connect times out is reported status: "unreachable".
//!
//! We use a non-routable address (192.0.2.1 — TEST-NET-1, RFC 5737) which
//! will never reply, causing a timeout. We use a short timeout to keep the test fast.

#[path = "helpers.rs"]
mod helpers;
use helpers::run_probe;
use mcp_cluster_health_monitor::probe::ClusterStatus;

fn blackhole_toml() -> String {
    // 192.0.2.1 is TEST-NET-1 (RFC 5737) — packets sent to it are silently dropped.
    // Port 9 is discard service; we use port 1 to be extra safe.
    r#"[[clusters]]
name = "unreachable-cluster"
endpoint = "192.0.2.1:1"
required = false
priority = 1
supported_backends = ["sql"]

[clusters.auth]
type = "direct"
pg_user = "PG_USER"
pg_pass_env = "PG_PASS"
"#
    .to_string()
}

#[tokio::test]
async fn ac2_timeout_reports_unreachable() {
    // Use a 200ms timeout to keep the test fast.
    let report = run_probe(&blackhole_toml(), 200).await;

    assert_eq!(report.clusters.len(), 1);
    let c = &report.clusters[0];
    assert_eq!(c.name, "unreachable-cluster");
    assert_eq!(c.status, ClusterStatus::Unreachable);
    assert!(c.latency_ms.is_none());
    assert!(c.error.is_some(), "error message should be populated");
}
