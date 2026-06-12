//! AC4: A cluster reported unhealthy has error populated; the current TCP-only probe
//! cannot confirm backends of an unhealthy cluster.
//!
//! The full backend-probe path (DAX/MDX round-trips) is deferred to a future iteration.
//! This test verifies that a cluster with status != healthy has error populated and
//! no latency — i.e. the "unconfirmed" semantics are preserved structurally.

#[path = "helpers.rs"]
mod helpers;
use helpers::{refused_port, run_probe};
use mcp_cluster_health_monitor::probe::ClusterStatus;

fn cluster_with_backend_toml(host_port: &str, backends: &[&str]) -> String {
    let backends_str = backends
        .iter()
        .map(|b| format!("\"{b}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"[[clusters]]
name = "multi-backend"
endpoint = "{host_port}"
required = false
priority = 1
supported_backends = [{backends_str}]

[clusters.auth]
type = "direct"
pg_user = "PG_USER"
pg_pass_env = "PG_PASS"
"#
    )
}

#[tokio::test]
async fn ac4_unhealthy_cluster_has_no_latency_and_has_error() {
    let refused = refused_port().await;
    let toml = cluster_with_backend_toml(&format!("127.0.0.1:{}", refused), &["sql", "dax"]);
    let report = run_probe(&toml, 2000).await;

    assert_eq!(report.clusters.len(), 1);
    let c = &report.clusters[0];
    assert_ne!(
        c.status,
        ClusterStatus::Healthy,
        "refused port should not be healthy"
    );
    assert!(
        c.latency_ms.is_none(),
        "unhealthy cluster should have no latency"
    );
    assert!(
        c.error.is_some(),
        "unhealthy cluster should have error message"
    );
}
