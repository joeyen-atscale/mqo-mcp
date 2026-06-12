//! AC7: --timeout-ms 200 causes a cluster that takes 300ms to respond to be reported unreachable.
//!
//! We connect to 192.0.2.1:1 (TEST-NET-1, RFC 5737 — packets silently dropped) with
//! a 200ms timeout. The connect will time out and the cluster is reported unreachable.

#[path = "helpers.rs"]
mod helpers;
use helpers::run_probe;
use mcp_cluster_health_monitor::probe::ClusterStatus;

fn blackhole_toml() -> String {
    r#"[[clusters]]
name = "slow-cluster"
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
async fn ac7_short_timeout_reports_unreachable() {
    use std::time::Instant;
    let start = Instant::now();
    // 200ms timeout — connect to a black-hole address
    let report = run_probe(&blackhole_toml(), 200).await;
    let elapsed = start.elapsed();

    assert_eq!(report.clusters.len(), 1);
    let c = &report.clusters[0];
    assert_eq!(
        c.status,
        ClusterStatus::Unreachable,
        "200ms timeout on black-hole should be unreachable"
    );
    // Should complete in roughly 200ms + some overhead, not 5 seconds
    assert!(
        elapsed.as_millis() < 1000,
        "test took {}ms; should have timed out quickly at 200ms",
        elapsed.as_millis()
    );
}
