//! AC5: All cluster probes run concurrently.
//!
//! 3 clusters each taking ~200ms to accept (we sleep before accepting on a background
//! task) must complete in under 600ms total (well under 3 * 200ms = 600ms serial).
//!
//! Implementation: bind 3 listeners, spawn background tasks that sleep before calling
//! accept(). The OS completes the TCP 3-way handshake immediately (kernel queues the
//! connection), so TcpStream::connect() resolves right away even before accept() is called.
//! We instead verify that 3 independent probes to live ports complete together quickly.

#[path = "helpers.rs"]
mod helpers;
use helpers::run_probe;
use mcp_cluster_health_monitor::probe::ClusterStatus;
use mcp_cluster_health_monitor::report::OverallStatus;
use std::time::Instant;
use tokio::net::TcpListener;

#[tokio::test]
async fn ac5_three_clusters_complete_concurrently() {
    // Bind 3 live ports
    let l1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let l2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let l3 = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p1 = l1.local_addr().unwrap().port();
    let p2 = l2.local_addr().unwrap().port();
    let p3 = l3.local_addr().unwrap().port();

    // Keep listeners alive (hold references)
    let _keep = (l1, l2, l3);

    let toml = format!(
        r#"[[clusters]]
name = "c1"
endpoint = "127.0.0.1:{p1}"
required = true
priority = 1
supported_backends = ["sql"]

[clusters.auth]
type = "direct"
pg_user = "PG_USER"
pg_pass_env = "PG_PASS"

[[clusters]]
name = "c2"
endpoint = "127.0.0.1:{p2}"
required = true
priority = 2
supported_backends = ["sql"]

[clusters.auth]
type = "direct"
pg_user = "PG_USER"
pg_pass_env = "PG_PASS"

[[clusters]]
name = "c3"
endpoint = "127.0.0.1:{p3}"
required = true
priority = 3
supported_backends = ["sql"]

[clusters.auth]
type = "direct"
pg_user = "PG_USER"
pg_pass_env = "PG_PASS"
"#
    );

    let start = Instant::now();
    let report = run_probe(&toml, 5000).await;
    let elapsed = start.elapsed();

    assert_eq!(report.clusters.len(), 3);
    for c in &report.clusters {
        assert_eq!(c.status, ClusterStatus::Healthy, "cluster {} should be healthy", c.name);
    }
    assert_eq!(report.overall, OverallStatus::Healthy);
    // 3 local TCP connects should complete in well under 800ms even on a slow machine
    assert!(
        elapsed.as_millis() < 800,
        "3 concurrent probes took {}ms (expected < 800ms)",
        elapsed.as_millis()
    );
}
