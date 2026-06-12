//! Health report types and aggregation logic.

use crate::probe::{ClusterHealth, ClusterStatus};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Overall health status of the fleet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverallStatus {
    /// All clusters healthy.
    Healthy,
    /// Some optional clusters down, all required clusters healthy.
    Degraded,
    /// At least one required cluster is unhealthy/unreachable.
    Critical,
}

impl std::fmt::Display for OverallStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OverallStatus::Healthy => write!(f, "healthy"),
            OverallStatus::Degraded => write!(f, "degraded"),
            OverallStatus::Critical => write!(f, "critical"),
        }
    }
}

/// Per-cluster report entry (serializable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterReport {
    pub name: String,
    pub status: ClusterStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Full health report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub timestamp_ms: u64,
    pub overall: OverallStatus,
    pub clusters: Vec<ClusterReport>,
}

impl HealthReport {
    /// Build a report from probe results.
    pub fn from_health(health: &[ClusterHealth]) -> Self {
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let clusters: Vec<ClusterReport> = health
            .iter()
            .map(|h| ClusterReport {
                name: h.name.clone(),
                status: h.status.clone(),
                latency_ms: h.latency_ms,
                error: h.error.clone(),
            })
            .collect();

        let overall = compute_overall(health);

        HealthReport {
            timestamp_ms,
            overall,
            clusters,
        }
    }
}

/// Compute overall status from individual probe results.
///
/// Rules:
/// - Any required cluster not healthy → Critical
/// - All required healthy, any optional not healthy → Degraded
/// - All healthy → Healthy
pub fn compute_overall(health: &[ClusterHealth]) -> OverallStatus {
    let any_required_down = health
        .iter()
        .any(|h| h.required && h.status != ClusterStatus::Healthy);

    if any_required_down {
        return OverallStatus::Critical;
    }

    let any_optional_down = health
        .iter()
        .any(|h| !h.required && h.status != ClusterStatus::Healthy);

    if any_optional_down {
        OverallStatus::Degraded
    } else {
        OverallStatus::Healthy
    }
}
