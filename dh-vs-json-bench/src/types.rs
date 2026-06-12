//! Core data types shared across the benchmark.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Task input ──────────────────────────────────────────────────────────────

/// A single benchmark task from the tasks JSON file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique task identifier.
    pub id: String,

    /// Natural-language question to answer over the fixture dataset.
    pub question: String,

    /// Name of the fixture dataset (e.g. `"sales"`).
    pub dataset: String,

    /// The known-correct answer value (serialized as a JSON value so it can
    /// represent scalars, arrays, or objects).
    pub correct_answer: Value,

    /// Optional description of what the correct answer represents.
    #[serde(default)]
    pub answer_description: Option<String>,
}

// ── Arm output ──────────────────────────────────────────────────────────────

/// Which arm produced this result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Arm {
    /// Arm A: raw-JSON — full rows handed to the model; model computes answer.
    RawJson,
    /// Arm B: handle — model gets summary+handle and uses `dataset_*` tools;
    /// answer is server-computed.
    Handle,
}

impl std::fmt::Display for Arm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Arm::RawJson => write!(f, "arm_a_raw_json"),
            Arm::Handle => write!(f, "arm_b_handle"),
        }
    }
}

/// Error class for a value error produced by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    /// Model performed arithmetic incorrectly on the provided rows.
    Arithmetic,
    /// Model copied/transcribed a number from the data but picked the wrong one.
    Transcription,
    /// Model answered over the wrong subset of the data.
    WrongSubset,
    /// Answer is correct.
    Correct,
}

impl std::fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorClass::Arithmetic => write!(f, "arithmetic"),
            ErrorClass::Transcription => write!(f, "transcription"),
            ErrorClass::WrongSubset => write!(f, "wrong_subset"),
            ErrorClass::Correct => write!(f, "correct"),
        }
    }
}

/// The raw output from one arm invocation, loaded from fixture or produced live.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArmOutput {
    /// Which arm.
    pub arm: Arm,

    /// Serialized description of what was sent to the model (rows JSON for
    /// arm A; handle + tool-call transcript for arm B).
    pub payload_summary: String,

    /// The answer the arm reported (from model for arm A; from server for arm B).
    pub reported_answer: Value,

    /// Error message, if any (e.g. tool-call failure, parse error).
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
}

// ── Grader verdict ──────────────────────────────────────────────────────────

/// The JSON shape the external grader must emit on stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraderVerdict {
    /// True if the reported answer matches the correct answer.
    pub correct: bool,

    /// Error class assigned by the grader.
    pub error_class: ErrorClass,

    /// Human-readable explanation (optional).
    #[serde(default)]
    pub reason: Option<String>,
}

// ── Per-question result ─────────────────────────────────────────────────────

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

    /// Grader verdict for arm A.
    pub arm_a_verdict: GraderVerdict,

    /// Grader verdict for arm B.
    pub arm_b_verdict: GraderVerdict,

    /// Whether arm A's answer is correct.
    pub arm_a_correct: bool,

    /// Whether arm B's answer is correct.
    pub arm_b_correct: bool,
}

// ── Aggregate report ────────────────────────────────────────────────────────

/// Aggregate benchmark report across all questions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::module_name_repetitions)]
pub struct AggregateReport {
    /// Total number of questions benchmarked.
    pub n_questions: usize,

    // --- Value-error rate (overall) ---
    /// Arm A value-error count (incorrect answers).
    pub arm_a_error_count: usize,
    /// Arm B value-error count.
    pub arm_b_error_count: usize,
    /// Arm A value-error rate (0.0–1.0).
    pub arm_a_error_rate: f64,
    /// Arm B value-error rate.
    pub arm_b_error_rate: f64,
    /// Value-error rate delta (arm_b − arm_a; negative = arm B is better).
    pub error_rate_delta: f64,
    /// Winner for value-error rate (lower is better).
    pub error_rate_winner: String,

    // --- Value-error rate by class ---
    /// Arm A arithmetic error count.
    pub arm_a_arithmetic_errors: usize,
    /// Arm B arithmetic error count.
    pub arm_b_arithmetic_errors: usize,
    /// Arithmetic error count delta (arm_b − arm_a).
    pub arithmetic_error_delta: f64,
    /// Arm A transcription error count.
    pub arm_a_transcription_errors: usize,
    /// Arm B transcription error count.
    pub arm_b_transcription_errors: usize,
    /// Transcription error count delta (arm_b − arm_a).
    pub transcription_error_delta: f64,
    /// Arm A wrong-subset error count.
    pub arm_a_wrong_subset_errors: usize,
    /// Arm B wrong-subset error count.
    pub arm_b_wrong_subset_errors: usize,
    /// Wrong-subset error count delta (arm_b − arm_a).
    pub wrong_subset_error_delta: f64,

    // --- Retries ---
    /// Arm A total retries.
    pub arm_a_total_retries: u64,
    /// Arm B total retries.
    pub arm_b_total_retries: u64,
    /// Arm A mean retries per question.
    pub arm_a_mean_retries: f64,
    /// Arm B mean retries per question.
    pub arm_b_mean_retries: f64,
    /// Retry delta (arm_b − arm_a).
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
    /// Latency delta ms (arm_b − arm_a).
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
    /// Token delta (arm_b − arm_a).
    pub token_delta: f64,
    /// Winner for token metric (lower is better).
    pub token_winner: String,
}

impl AggregateReport {
    /// Return a zeroed aggregate report (used when the results slice is empty).
    #[must_use]
    pub fn zero() -> Self {
        Self {
            n_questions: 0,
            arm_a_error_count: 0,
            arm_b_error_count: 0,
            arm_a_error_rate: 0.0,
            arm_b_error_rate: 0.0,
            error_rate_delta: 0.0,
            error_rate_winner: "tie".to_string(),
            arm_a_arithmetic_errors: 0,
            arm_b_arithmetic_errors: 0,
            arithmetic_error_delta: 0.0,
            arm_a_transcription_errors: 0,
            arm_b_transcription_errors: 0,
            transcription_error_delta: 0.0,
            arm_a_wrong_subset_errors: 0,
            arm_b_wrong_subset_errors: 0,
            wrong_subset_error_delta: 0.0,
            arm_a_total_retries: 0,
            arm_b_total_retries: 0,
            arm_a_mean_retries: 0.0,
            arm_b_mean_retries: 0.0,
            retry_delta: 0.0,
            retry_winner: "tie".to_string(),
            arm_a_total_latency_ms: 0,
            arm_b_total_latency_ms: 0,
            arm_a_mean_latency_ms: 0.0,
            arm_b_mean_latency_ms: 0.0,
            latency_delta_ms: 0.0,
            latency_winner: "tie".to_string(),
            arm_a_total_tokens: 0,
            arm_b_total_tokens: 0,
            arm_a_mean_tokens: 0.0,
            arm_b_mean_tokens: 0.0,
            token_delta: 0.0,
            token_winner: "tie".to_string(),
        }
    }
}
