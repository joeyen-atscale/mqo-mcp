use serde::{Deserialize, Serialize};

/// Input — emitted by mqo-bench --output json
#[derive(Debug, Deserialize)]
pub struct BenchReport {
    pub aggregate: AggMetrics,
    pub per_question: Vec<QuestionResult>,
}

#[derive(Debug, Deserialize)]
pub struct QuestionResult {
    // flexible — we only count them
    #[serde(flatten)]
    pub _extra: serde_json::Value,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AggMetrics {
    pub accuracy_delta_pp: f64,
    pub entity_error_delta_pp: f64,
    pub latency_delta_ms: f64,
    pub token_delta: f64,
}

/// History store — one JSON object per line in runs.jsonl
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HistoryRecord {
    pub run_id: String,
    pub timestamp: String,
    pub aggregate: AggMetrics,
    pub per_question_count: usize,
    pub task_file_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Warn,
    Regress,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verdict::Ok => write!(f, "OK"),
            Verdict::Warn => write!(f, "WARN"),
            Verdict::Regress => write!(f, "REGRESS"),
        }
    }
}
