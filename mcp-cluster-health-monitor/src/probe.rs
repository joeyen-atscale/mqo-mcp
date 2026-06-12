//! TCP probe logic — one task per cluster, concurrent via tokio.

use mcp_cluster_registry::ClusterEntry;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::time::timeout;

/// Per-cluster probe status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClusterStatus {
    /// TCP connect succeeded within timeout.
    Healthy,
    /// TCP connect timed out.
    Unreachable,
    /// TCP connect was actively refused (connection refused).
    Unhealthy,
}

impl std::fmt::Display for ClusterStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClusterStatus::Healthy => write!(f, "healthy"),
            ClusterStatus::Unreachable => write!(f, "unreachable"),
            ClusterStatus::Unhealthy => write!(f, "unhealthy"),
        }
    }
}

/// Result of probing a single cluster.
#[derive(Debug, Clone)]
pub struct ClusterHealth {
    pub name: String,
    pub required: bool,
    pub status: ClusterStatus,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

/// Probe a single cluster endpoint via TCP connect.
async fn probe_one(entry: &ClusterEntry, timeout_ms: u64) -> ClusterHealth {
    let endpoint = &entry.endpoint;

    let start = Instant::now();
    let connect_result = timeout(
        Duration::from_millis(timeout_ms),
        TcpStream::connect(endpoint),
    )
    .await;

    match connect_result {
        Ok(Ok(_stream)) => {
            // TCP connect succeeded
            let latency = start.elapsed().as_millis() as u64;
            ClusterHealth {
                name: entry.name.clone(),
                required: entry.required,
                status: ClusterStatus::Healthy,
                latency_ms: Some(latency),
                error: None,
            }
        }
        Ok(Err(e)) => {
            // TCP connect failed (refused, reset, etc.)
            ClusterHealth {
                name: entry.name.clone(),
                required: entry.required,
                status: ClusterStatus::Unhealthy,
                latency_ms: None,
                error: Some(e.to_string()),
            }
        }
        Err(_elapsed) => {
            // Timed out
            ClusterHealth {
                name: entry.name.clone(),
                required: entry.required,
                status: ClusterStatus::Unreachable,
                latency_ms: None,
                error: Some(format!("TCP connect timed out after {timeout_ms}ms")),
            }
        }
    }
}

/// Probe all clusters concurrently, collecting results.
pub async fn probe_clusters(
    clusters: &[&ClusterEntry],
    timeout_ms: u64,
) -> Vec<ClusterHealth> {
    let tasks: Vec<_> = clusters
        .iter()
        .map(|entry| {
            let entry = (*entry).clone();
            let ms = timeout_ms;
            tokio::spawn(async move { probe_one(&entry, ms).await })
        })
        .collect();

    let mut results = Vec::with_capacity(tasks.len());
    for task in tasks {
        match task.await {
            Ok(health) => results.push(health),
            Err(e) => {
                // Task panicked — shouldn't happen, but handle gracefully
                results.push(ClusterHealth {
                    name: "unknown".to_string(),
                    required: false,
                    status: ClusterStatus::Unhealthy,
                    latency_ms: None,
                    error: Some(format!("probe task panicked: {e}")),
                });
            }
        }
    }
    results
}
