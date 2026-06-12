//! Output report types for the cross-cluster diff.

use serde::{Deserialize, Serialize};

/// Verdict for a single entity comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// All compared fields match (within tolerance for numeric fields).
    Agree,
    /// One or more non-critical fields differ.
    Diverge,
    /// `expression` or `aggregation_type` differs — semantic change.
    CriticalDiverge,
}

/// A single field difference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDiff {
    pub field: String,
    pub cluster_a: Option<String>,
    pub cluster_b: Option<String>,
    pub critical: bool,
}

/// Overall verdict for the diff run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverallVerdict {
    Agree,
    Diverge,
    CriticalDiverge,
}

/// One entity's comparison record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityDiff {
    pub entity_type: String, // "measure" or "dimension"
    pub unique_name: String,
    pub verdict: Verdict,
    pub field_diffs: Vec<FieldDiff>,
}

/// Summary counts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Summary {
    pub agree: usize,
    pub diverge: usize,
    pub critical_diverge: usize,
    pub only_in_a: usize,
    pub only_in_b: usize,
}

/// Top-level diff report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffReport {
    pub clusters: ClusterInfo,
    pub summary: Summary,
    pub differences: Vec<EntityDiff>,
    pub only_in_a: Vec<OnlyEntry>,
    pub only_in_b: Vec<OnlyEntry>,
    pub overall_verdict: OverallVerdict,
}

/// Cluster name pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterInfo {
    pub a: String,
    pub b: String,
}

/// An entity present in only one cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnlyEntry {
    pub entity_type: String,
    pub unique_name: String,
}
