//! Core data types shared across the benchmark.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Task input ─────────────────────────────────────────────────────────────

/// A single benchmark task from the tasks JSON file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique task identifier.
    pub id: String,

    /// Natural-language question to answer.
    pub question: String,

    /// The target semantic model name (e.g. `"sales"`).
    pub model: String,

    /// Optional expected SQL (for arm A reference).
    #[serde(default)]
    pub reference_sql: Option<String>,

    /// Optional expected MQO (for arm B reference).
    #[serde(default)]
    pub reference_mqo: Option<Value>,
}

// ── Arm output ─────────────────────────────────────────────────────────────

/// Which arm produced this result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arm {
    /// Arm A: text-to-SQL → `run_query`.
    SqlRunQuery,
    /// Arm B: MQO → `query_multidimensional`.
    MqoMultidimensional,
}

impl std::fmt::Display for Arm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Arm::SqlRunQuery => write!(f, "arm_a_sql"),
            Arm::MqoMultidimensional => write!(f, "arm_b_mqo"),
        }
    }
}

/// The raw output from one arm invocation, loaded from fixture or produced live.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArmOutput {
    /// Which arm.
    pub arm: Arm,

    /// The SQL or serialised MQO payload that was sent.
    pub query: String,

    /// Result rows as returned by the engine (may be empty on error).
    #[serde(default)]
    pub rows: Vec<Value>,

    /// Error message, if any.
    #[serde(default)]
    pub error: Option<String>,

    /// Number of retries (0 = single-shot).
    #[serde(default)]
    pub n_retries: u32,

    /// Wall-clock latency in milliseconds.
    #[serde(default)]
    pub latency_ms: u64,

    /// Input + output tokens consumed.
    #[serde(default)]
    pub tokens: u64,

    /// True if the error or response text indicates an invalid entity
    /// (hallucinated measure/dimension name) error from the engine.
    #[serde(default)]
    pub invalid_entity: bool,
}

// ── Grader verdict ─────────────────────────────────────────────────────────

/// The JSON shape the external grader must emit on stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraderVerdict {
    /// True if the two result sets are equivalent.
    pub equivalent: bool,

    /// Human-readable explanation (optional).
    #[serde(default)]
    pub reason: Option<String>,
}

// ── Per-question result ────────────────────────────────────────────────────

/// Per-question head-to-head result row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionResult {
    /// Task id.
    pub task_id: String,

    /// The natural-language question.
    pub question: String,

    /// Arm A output.
    pub arm_a: ArmOutput,

    /// Arm B output.
    pub arm_b: ArmOutput,

    /// Whether arm A's result is equivalent to arm B's (per grader).
    pub arms_equivalent: bool,

    /// Whether arm A passed (no error, equivalent to reference if available,
    /// or equal to arm B on non-reference tasks).
    pub arm_a_pass: bool,

    /// Whether arm B passed.
    pub arm_b_pass: bool,

    /// Grader verdict text.
    #[serde(default)]
    pub grader_reason: Option<String>,
}

// ── Aggregate report ───────────────────────────────────────────────────────

/// Aggregate benchmark report across all questions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateReport {
    /// Total number of questions benchmarked.
    pub n_questions: usize,

    // --- Accuracy ---
    /// Arm A pass count.
    pub arm_a_pass_count: usize,
    /// Arm B pass count.
    pub arm_b_pass_count: usize,
    /// Arm A accuracy (0.0–1.0).
    pub arm_a_accuracy: f64,
    /// Arm B accuracy (0.0–1.0).
    pub arm_b_accuracy: f64,
    /// Accuracy delta (arm_b − arm_a).
    pub accuracy_delta: f64,
    /// Winner for accuracy metric.
    pub accuracy_winner: String,

    // --- Invalid-entity (hallucination) rate ---
    /// Arm A invalid-entity count.
    pub arm_a_invalid_entity_count: usize,
    /// Arm B invalid-entity count.
    pub arm_b_invalid_entity_count: usize,
    /// Arm A invalid-entity rate.
    pub arm_a_invalid_entity_rate: f64,
    /// Arm B invalid-entity rate.
    pub arm_b_invalid_entity_rate: f64,
    /// Invalid-entity delta (arm_b − arm_a; negative = arm B is better).
    pub invalid_entity_delta: f64,
    /// Winner for invalid-entity metric (lower is better).
    pub invalid_entity_winner: String,

    // --- Retries ---
    /// Arm A total retries.
    pub arm_a_total_retries: u64,
    /// Arm B total retries.
    pub arm_b_total_retries: u64,
    /// Arm A mean retries per question.
    pub arm_a_mean_retries: f64,
    /// Arm B mean retries per question.
    pub arm_b_mean_retries: f64,
    /// Retry delta.
    pub retry_delta: f64,
    /// Winner for retry metric (lower is better).
    pub retry_winner: String,

    // --- Latency ---
    /// Arm A total latency ms.
    pub arm_a_total_latency_ms: u64,
    /// Arm B total latency ms.
    pub arm_b_total_latency_ms: u64,
    /// Arm A mean latency ms.
    pub arm_a_mean_latency_ms: f64,
    /// Arm B mean latency ms.
    pub arm_b_mean_latency_ms: f64,
    /// Latency delta ms.
    pub latency_delta_ms: f64,
    /// Winner for latency metric (lower is better).
    pub latency_winner: String,

    // --- Tokens ---
    /// Arm A total tokens.
    pub arm_a_total_tokens: u64,
    /// Arm B total tokens.
    pub arm_b_total_tokens: u64,
    /// Arm A mean tokens per question.
    pub arm_a_mean_tokens: f64,
    /// Arm B mean tokens per question.
    pub arm_b_mean_tokens: f64,
    /// Token delta.
    pub token_delta: f64,
    /// Winner for token metric (lower is better).
    pub token_winner: String,
}
