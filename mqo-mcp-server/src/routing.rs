//! Cluster routing logic for federation mode.
//!
//! When `--registry` is active, incoming `query_multidimensional` calls may
//! carry an optional `cluster` field. This module selects the target cluster:
//!
//! - `preferred_cluster = Some("name")` — use that cluster directly (or error
//!   if it does not exist in the registry).
//! - `preferred_cluster = None` — iterate clusters sorted by ascending priority,
//!   picking the first one whose status is `Healthy` in the latest health cache.
//!
//! The health cache is a `HealthReport` built from a previous TCP probe run.
//! Staleness is accepted: the caller controls when to refresh via
//! `health_status` tool.

use mcp_cluster_health_monitor::probe::ClusterStatus;
use mcp_cluster_health_monitor::report::HealthReport;
use mcp_cluster_registry::{ClusterEntry, ClusterRegistry};
use thiserror::Error;

/// Errors produced during cluster selection.
#[derive(Debug, Error)]
pub enum RoutingError {
    /// The caller named a specific cluster that is not in the registry.
    #[error("cluster '{0}' not found in registry")]
    NotFound(String),

    /// No registry was loaded; federation routing is unavailable.
    #[error("no registry configured")]
    NoRegistry,

    /// Every cluster in the registry is unhealthy/unreachable.
    #[error("no healthy cluster available")]
    NoneHealthy,
}

/// Select a cluster entry from the registry.
///
/// - If `preferred_cluster` is `Some`, look it up by name and return it (no
///   health filter — the caller explicitly asked for it; let the engine fail
///   with a real error if unreachable).
/// - If `preferred_cluster` is `None`, return the highest-priority cluster
///   (lowest `priority` value) whose name appears as `Healthy` in `health`.
///   If there is no health report yet, fall back to the highest-priority
///   cluster unconditionally.
///
/// # Errors
///
/// Returns `RoutingError::NotFound` when a named cluster is absent, or
/// `RoutingError::NoneHealthy` when auto-routing finds no live candidate.
pub fn select_cluster<'a>(
    registry: &'a ClusterRegistry,
    health: Option<&HealthReport>,
    preferred_cluster: Option<&str>,
) -> Result<&'a ClusterEntry, RoutingError> {
    if let Some(name) = preferred_cluster {
        return registry
            .get(name)
            .ok_or_else(|| RoutingError::NotFound(name.to_string()));
    }

    // Auto-route: highest-priority healthy cluster.
    let sorted = registry.by_priority();

    if let Some(report) = health {
        // Filter to clusters that appear healthy in the cached report.
        let healthy: Vec<&ClusterEntry> = sorted
            .iter()
            .filter(|c| {
                report
                    .clusters
                    .iter()
                    .any(|cr| cr.name == c.name && cr.status == ClusterStatus::Healthy)
            })
            .copied()
            .collect();

        if let Some(&entry) = healthy.first() {
            return Ok(entry);
        }
        return Err(RoutingError::NoneHealthy);
    }

    // No health report yet — return the highest-priority cluster.
    sorted
        .into_iter()
        .next()
        .ok_or(RoutingError::NoneHealthy)
}

/// Run a synchronous health check against all clusters in the registry,
/// returning a fresh `HealthReport`. Uses a dedicated Tokio runtime so it
/// can be called from a sync context.
///
/// # Panics
///
/// Panics if the Tokio single-threaded runtime cannot be built (should only
/// happen in extremely resource-constrained environments).
#[must_use = "health report should be stored or returned"]
pub fn run_health_check_sync(registry: &ClusterRegistry, timeout_ms: u64) -> HealthReport {
    let clusters: Vec<&ClusterEntry> = registry.clusters.iter().collect();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let health = rt.block_on(mcp_cluster_health_monitor::probe::probe_clusters(
        &clusters,
        timeout_ms,
    ));
    HealthReport::from_health(&health)
}
