//! AC1: A cluster whose TCP connect succeeds is reported status: "healthy" with latency_ms > 0.

#[path = "helpers.rs"]
mod helpers;
use helpers::{bind_free_port, run_probe, single_cluster_toml};
use mcp_cluster_health_monitor::probe::ClusterStatus;

#[tokio::test]
async fn ac1_tcp_connect_succeeds_reports_healthy() {
    let (_listener, port) = bind_free_port().await;
    let toml = single_cluster_toml("prod", &format!("127.0.0.1:{}", port), true);
    let report = run_probe(&toml, 5000).await;

    assert_eq!(report.clusters.len(), 1);
    let c = &report.clusters[0];
    assert_eq!(c.name, "prod");
    assert_eq!(c.status, ClusterStatus::Healthy);
    let latency = c.latency_ms.expect("latency_ms must be Some for healthy cluster");
    assert!(latency < 2000, "latency should be < 2s, got {}ms", latency);
    assert!(c.error.is_none());
}
