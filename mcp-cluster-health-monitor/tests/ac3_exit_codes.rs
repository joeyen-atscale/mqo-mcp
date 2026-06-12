//! AC3: Exit-code and overall-status logic based on required vs optional clusters.
//!
//! - required cluster unreachable → overall: critical
//! - optional cluster unreachable, required healthy → overall: degraded

#[path = "helpers.rs"]
mod helpers;
use helpers::{bind_free_port, refused_port, run_probe};
use mcp_cluster_health_monitor::report::OverallStatus;

fn two_cluster_toml(
    name_a: &str,
    endpoint_a: &str,
    required_a: bool,
    name_b: &str,
    endpoint_b: &str,
    required_b: bool,
) -> String {
    format!(
        r#"[[clusters]]
name = "{name_a}"
endpoint = "{endpoint_a}"
required = {required_a}
priority = 1
supported_backends = ["sql"]

[clusters.auth]
type = "direct"
pg_user = "PG_USER"
pg_pass_env = "PG_PASS"

[[clusters]]
name = "{name_b}"
endpoint = "{endpoint_b}"
required = {required_b}
priority = 2
supported_backends = ["sql"]

[clusters.auth]
type = "direct"
pg_user = "PG_USER"
pg_pass_env = "PG_PASS"
"#
    )
}

#[tokio::test]
async fn ac3_required_cluster_down_is_critical() {
    // Required cluster: use a refused port (unhealthy)
    let refused = refused_port().await;
    // Optional cluster: live listener (healthy)
    let (_listener, live_port) = bind_free_port().await;

    let toml = two_cluster_toml(
        "prod",
        &format!("127.0.0.1:{}", refused),
        true,
        "staging",
        &format!("127.0.0.1:{}", live_port),
        false,
    );
    let report = run_probe(&toml, 2000).await;
    assert_eq!(
        report.overall,
        OverallStatus::Critical,
        "required cluster down → critical; got {:?}",
        report.overall
    );
}

#[tokio::test]
async fn ac3_optional_cluster_down_required_healthy_is_degraded() {
    // Required cluster: live listener (healthy)
    let (_listener, live_port) = bind_free_port().await;
    // Optional cluster: refused (unhealthy)
    let refused = refused_port().await;

    let toml = two_cluster_toml(
        "prod",
        &format!("127.0.0.1:{}", live_port),
        true,
        "staging",
        &format!("127.0.0.1:{}", refused),
        false,
    );
    let report = run_probe(&toml, 2000).await;
    assert_eq!(
        report.overall,
        OverallStatus::Degraded,
        "optional cluster down, required healthy → degraded; got {:?}",
        report.overall
    );
}
