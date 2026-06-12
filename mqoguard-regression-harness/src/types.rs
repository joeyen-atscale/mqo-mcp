//! Shared data types for corpus, trajectory records, baseline, and report.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---- Corpus types ----

/// Top-level corpus JSON (`tpcds_failure_modes_100_nonprod.json`).
#[derive(Debug, Deserialize)]
pub struct Corpus {
    /// List of tasks.
    pub tasks: Vec<Task>,
}

/// A single task from the corpus.
#[derive(Debug, Deserialize, Clone)]
pub struct Task {
    /// Unique task identifier (e.g. "fm1-001").
    pub id: String,

    /// Failure mode tag.
    pub failure_mode: Option<String>,

    /// Canonical block: measures, dimensions, approach.
    pub canonical: Option<CanonicalBlock>,

    /// Rejected pairings / constructs.
    #[serde(default)]
    pub rejected: Vec<String>,

    /// Pre-normalised required calcs (calc-sensitive corpus shape).
    pub required_calcs: Option<Vec<String>>,

    /// Pre-normalised required dims (calc-sensitive corpus shape).
    pub required_dims: Option<Vec<String>>,

    /// Pre-normalised forbidden constructs.
    pub forbidden_constructs: Option<Vec<String>>,

    /// Also-acceptable calcs (supplement to `required_calcs`).
    #[serde(default)]
    pub also_acceptable_calcs: Vec<String>,

    /// Minimum expected row count (default 1; 0 means agent must NOT return rows).
    pub expected_min_rows: Option<i64>,

    /// Expected numeric value in first numeric column of first row.
    pub expected_numeric: Option<f64>,
}

/// Canonical block inside a task.
#[derive(Debug, Deserialize, Clone)]
pub struct CanonicalBlock {
    /// Canonical approach text (e.g. "reject the query").
    pub approach: Option<String>,

    /// Canonical measures.
    #[serde(default)]
    pub measures: Vec<String>,

    /// Canonical dimensions.
    #[serde(default)]
    pub dimensions: Vec<String>,
}

// ---- Trajectory record ----

/// One row from `trajectories.jsonl`.
#[derive(Debug, Deserialize, Clone)]
pub struct TrajectoryRecord {
    /// Task ID from corpus.
    pub task_id: String,

    /// MCP arm tag (e.g. "nonprod", "prod").
    pub mcp: Option<String>,

    /// Rollout index (0-based).
    pub rollout: Option<i64>,

    /// Model family.
    pub model: Option<String>,

    /// Exact model version string.
    pub model_id: Option<String>,

    /// Final SQL authored by the agent (may be empty/null).
    pub final_sql: Option<String>,

    /// Rows returned from `run_query` (array of objects).
    pub rows: Option<Vec<HashMap<String, serde_json::Value>>>,

    /// Row count from `run_query`.
    pub row_count: Option<i64>,

    /// Error string from `run_query`, if any.
    pub error: Option<String>,

    /// Agent's natural-language answer.
    pub answer: Option<String>,

    /// Number of retries (single-shot invariant: should be 0).
    pub n_retries: Option<i64>,
}

// ---- Baseline ----

/// Baseline file: per-mode floors.
#[derive(Debug, Deserialize)]
pub struct Baseline {
    /// Description / provenance note.
    pub description: Option<String>,

    /// Per-mode floor entries keyed by `failure_mode` string.
    pub modes: HashMap<String, ModeFloor>,

    /// Overall path-mean floor (optional).
    pub overall_path_mean_floor: Option<f64>,

    /// Seeded from which run.
    pub seeded_from: Option<String>,
}

/// Floor entry for one failure mode.
#[derive(Debug, Deserialize)]
pub struct ModeFloor {
    /// Path-mean floor (0.0–1.0).
    pub path_mean_floor: Option<f64>,

    /// Pass-at-k floor (0.0–1.0).
    pub pass_at_k_floor: Option<f64>,

    /// Human note.
    pub note: Option<String>,
}

// ---- Scoring result types ----

/// Score for one trajectory record.
#[derive(Debug, Clone)]
pub struct RecordScore {
    /// Task ID.
    pub task_id: String,

    /// MCP arm.
    pub mcp: Option<String>,

    /// Rollout index.
    pub rollout: Option<i64>,

    /// Did the record pass path correctness?
    pub pass_by_path: bool,

    /// Why (short failure reason for bucketing).
    pub why_path: String,
}

/// Per-mode aggregate scores.
#[derive(Debug, Clone, Serialize)]
pub struct ModeScore {
    /// Failure mode name.
    pub mode: String,

    /// Number of unique tasks in this mode.
    pub n_tasks: usize,

    /// Mean pass rate across tasks (averaging over rollouts per task).
    pub path_mean: f64,

    /// Pass-at-k: fraction of tasks where at least one rollout passed.
    pub pass_at_k: f64,

    /// Average k (rollouts per task).
    pub avg_k: f64,

    /// Per-failure-reason counts (across all rollouts).
    pub failure_reasons: HashMap<String, usize>,

    // Gate fields (set by `report::build_report`)

    /// Floor from baseline (`None` if mode absent from baseline).
    pub path_mean_floor: Option<f64>,

    /// Pass-at-k floor from baseline.
    pub pass_at_k_floor: Option<f64>,

    /// Whether this mode is gated (floor present in baseline).
    pub is_gated: bool,

    /// Whether `path_mean` meets floor (after tolerance).
    pub path_mean_ok: bool,

    /// Delta from floor (positive = above floor).
    pub path_mean_delta: Option<f64>,
}

/// Overall summary line.
#[derive(Debug, Clone, Serialize)]
pub struct OverallScore {
    /// Total tasks scored.
    pub n_tasks: usize,

    /// Total records scored.
    pub n_records: usize,

    /// Overall path-mean across all modes.
    pub path_mean: f64,

    /// Overall pass-at-k across all modes.
    pub pass_at_k: f64,

    /// Whether any gated mode is below its floor.
    pub any_below_floor: bool,

    /// Names of modes that are below their floor.
    pub failing_modes: Vec<String>,
}

/// Full report emitted in JSON mode.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Per-mode scores.
    pub modes: Vec<ModeScore>,

    /// Overall summary.
    pub overall: OverallScore,

    /// Tolerance applied to floors.
    pub tolerance: f64,
}
