//! `mcp-interaction-scorer` — compute per-session and per-entity quality distributions
//! from a durable JSONL trace store.
//!
//! This crate reads a JSONL file whose records conform to the [`mcp_trace_store::TraceRecord`]
//! schema and produces two maps:
//!
//! - [`SessionQuality`]: per-`session_id` aggregated rates (retry rate, empty-result rate,
//!   bind-failure rate).
//! - [`EntityQuality`]: per-entity-name distributions (first-attempt bind rate,
//!   retry histogram, result fill rate, grounding score distribution).
//!
//! # Usage
//! ```no_run
//! use mcp_interaction_scorer::score_trace_store;
//!
//! let report = score_trace_store("/path/to/trace.jsonl").unwrap();
//! for (session_id, quality) in &report.sessions {
//!     println!("{session_id}: retry_rate={:.3}", quality.retry_rate);
//! }
//! ```
#![deny(unsafe_code)]

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use mcp_trace_store::{BindOutcome, ExecuteOutcome, TraceRecord};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur while scoring a trace store.
#[derive(Debug, thiserror::Error)]
pub enum ScorerError {
    /// The file could not be opened or read.
    #[error("io error: {0}")]
    IoError(#[from] io::Error),

    /// A JSONL line could not be parsed; carries the 1-based line number.
    #[error("parse error at line {line_number}: {message}")]
    ParseError {
        /// 1-based line number of the offending record.
        line_number: usize,
        /// Human-readable description of the parse failure.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// Quality metrics aggregated over a single MCP session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionQuality {
    /// Session identifier (mirrors `TraceRecord::session_id`).
    pub session_id: String,
    /// Fraction of MQOs that required at least one retry (`bind_attempt_count > 1`).
    /// `retried_mqos / total_mqos`.
    pub retry_rate: f64,
    /// Fraction of MQOs whose execute result was `Success { result_empty: true }`.
    /// `empty_result_mqos / total_mqos`.
    pub empty_result_rate: f64,
    /// Fraction of MQOs whose bind outcome was not `Success`.
    /// `failed_binds / total_mqos`.
    pub bind_failure_rate: f64,
    /// Total number of MQO interactions in this session.
    pub total_mqos: u64,
}

/// Quality distributions for a single named model entity (measure or dimension).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityQuality {
    /// Name of the entity as it appears in the MQO `unique_name` fields.
    pub entity_name: String,
    /// Fraction of interactions referencing this entity where the bind succeeded on the
    /// first attempt (`quality.first_attempt_bind == true`).
    pub first_attempt_bind_rate: f64,
    /// Vector of per-interaction `bind_attempt_count` values (the retry histogram).
    /// Callers may bucket this as needed.
    pub retry_histogram: Vec<u8>,
    /// Fraction of interactions referencing this entity that returned a non-empty result.
    /// `filled_results / total_interactions`.
    pub result_fill_rate: f64,
    /// Raw `grounding_score` values for interactions referencing this entity.
    /// Full fidelity is preserved; callers bin as needed.
    pub grounding_scores: Vec<f64>,
    /// Total number of interactions referencing this entity.
    pub total_interactions: u64,
}

/// The scored output produced by [`score_trace_store`] or [`score_reader`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScorerReport {
    /// Per-session quality metrics, keyed by `session_id`.
    pub sessions: HashMap<String, SessionQuality>,
    /// Per-entity quality distributions, keyed by entity `unique_name`.
    pub entities: HashMap<String, EntityQuality>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Score a JSONL trace store located at `path`.
///
/// Returns a [`ScorerReport`] containing per-session and per-entity quality metrics.
///
/// # Errors
/// - [`ScorerError::IoError`] if the file cannot be opened or read.
/// - [`ScorerError::ParseError`] if any line is not valid UTF-8 or not valid JSON
///   matching the `TraceRecord` schema; carries the 1-based line number.
pub fn score_trace_store(path: impl AsRef<Path>) -> Result<ScorerReport, ScorerError> {
    let file = std::fs::File::open(path.as_ref())?;
    let reader = BufReader::new(file);
    score_reader(reader)
}

/// Score a JSONL trace store from any [`BufRead`] source.
///
/// This variant accepts any buffered reader — e.g. a [`std::io::Cursor`] wrapping
/// an in-memory buffer — enabling testing and streaming use without touching the
/// filesystem.
///
/// # Errors
/// - [`ScorerError::IoError`] if a line cannot be read from the reader.
/// - [`ScorerError::ParseError`] if any line is not valid JSON matching the
///   `TraceRecord` schema; carries the 1-based line number.
pub fn score_reader(reader: impl BufRead) -> Result<ScorerReport, ScorerError> {
    // Accumulators — indexed by session_id / entity_name.
    let mut session_acc: HashMap<String, SessionAccumulator> = HashMap::new();
    let mut entity_acc: HashMap<String, EntityAccumulator> = HashMap::new();

    for (idx, line_result) in reader.lines().enumerate() {
        let line_number = idx + 1; // 1-based

        let line = line_result.map_err(ScorerError::IoError)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let record: TraceRecord = serde_json::from_str(trimmed).map_err(|e| {
            ScorerError::ParseError {
                line_number,
                message: e.to_string(),
            }
        })?;

        accumulate_session(&mut session_acc, &record);
        accumulate_entities(&mut entity_acc, &record);
    }

    let sessions: HashMap<String, SessionQuality> = session_acc
        .into_values()
        .map(|acc| (acc.session_id.clone(), acc.into_quality()))
        .collect();

    let entities: HashMap<String, EntityQuality> = entity_acc
        .into_values()
        .map(|acc| (acc.entity_name.clone(), acc.into_quality()))
        .collect();

    Ok(ScorerReport { sessions, entities })
}

// ---------------------------------------------------------------------------
// Internal accumulators
// ---------------------------------------------------------------------------

/// Mutable accumulator for a single session's quality metrics.
#[derive(Debug, Default)]
struct SessionAccumulator {
    session_id: String,
    total_mqos: u64,
    retried_mqos: u64,
    empty_result_mqos: u64,
    failed_binds: u64,
}

impl SessionAccumulator {
    fn new(session_id: &str) -> Self {
        SessionAccumulator {
            session_id: session_id.to_owned(),
            ..Default::default()
        }
    }

    fn into_quality(self) -> SessionQuality {
        let n = self.total_mqos as f64;
        let (retry_rate, empty_result_rate, bind_failure_rate) = if n == 0.0 {
            (0.0, 0.0, 0.0)
        } else {
            (
                self.retried_mqos as f64 / n,
                self.empty_result_mqos as f64 / n,
                self.failed_binds as f64 / n,
            )
        };
        SessionQuality {
            session_id: self.session_id,
            retry_rate,
            empty_result_rate,
            bind_failure_rate,
            total_mqos: self.total_mqos,
        }
    }
}

/// Mutable accumulator for a single entity's quality distributions.
#[derive(Debug, Default)]
struct EntityAccumulator {
    entity_name: String,
    total_interactions: u64,
    first_attempt_successes: u64,
    retry_histogram: Vec<u8>,
    filled_results: u64,
    grounding_scores: Vec<f64>,
}

impl EntityAccumulator {
    fn new(entity_name: &str) -> Self {
        EntityAccumulator {
            entity_name: entity_name.to_owned(),
            ..Default::default()
        }
    }

    fn into_quality(self) -> EntityQuality {
        let n = self.total_interactions as f64;
        let first_attempt_bind_rate = if n == 0.0 {
            0.0
        } else {
            self.first_attempt_successes as f64 / n
        };
        let result_fill_rate = if n == 0.0 {
            0.0
        } else {
            self.filled_results as f64 / n
        };
        EntityQuality {
            entity_name: self.entity_name,
            first_attempt_bind_rate,
            retry_histogram: self.retry_histogram,
            result_fill_rate,
            grounding_scores: self.grounding_scores,
            total_interactions: self.total_interactions,
        }
    }
}

// ---------------------------------------------------------------------------
// Accumulation helpers
// ---------------------------------------------------------------------------

fn accumulate_session(acc: &mut HashMap<String, SessionAccumulator>, record: &TraceRecord) {
    let entry = acc
        .entry(record.session_id.clone())
        .or_insert_with(|| SessionAccumulator::new(&record.session_id));

    entry.total_mqos += 1;

    // Retried = more than one bind attempt.
    if record.quality.bind_attempt_count > 1 {
        entry.retried_mqos += 1;
    }

    // Empty result.
    if let ExecuteOutcome::Success {
        result_empty: true, ..
    } = &record.execute_result
    {
        entry.empty_result_mqos += 1;
    }

    // Bind failure = outcome is not Success.
    if !matches!(record.bind_outcome, BindOutcome::Success) {
        entry.failed_binds += 1;
    }
}

/// Extract entity names from the MQO JSON value.
///
/// The MQO is a JSON object with optional `measures` and `dimensions` arrays.
/// Each measure element has a `unique_name` string.
/// Each dimension element is a level selection that may carry a `unique_name`.
fn entity_names_from_mqo(mqo: &serde_json::Value) -> Vec<String> {
    let mut names = Vec::new();

    // Measures: [{unique_name: "..."}, ...]
    if let Some(measures) = mqo.get("measures").and_then(|v| v.as_array()) {
        for m in measures {
            if let Some(name) = m.get("unique_name").and_then(|v| v.as_str()) {
                names.push(name.to_owned());
            }
        }
    }

    // Dimensions: [{unique_name: "..."}, ...] or [{level: {unique_name: "..."}}, ...]
    if let Some(dims) = mqo.get("dimensions").and_then(|v| v.as_array()) {
        for d in dims {
            if let Some(name) = d.get("unique_name").and_then(|v| v.as_str()) {
                names.push(name.to_owned());
            }
            // Also check nested level unique_name.
            if let Some(name) = d
                .get("level")
                .and_then(|l| l.get("unique_name"))
                .and_then(|v| v.as_str())
            {
                names.push(name.to_owned());
            }
        }
    }

    names
}

fn accumulate_entities(
    acc: &mut HashMap<String, EntityAccumulator>,
    record: &TraceRecord,
) {
    let names = entity_names_from_mqo(&record.mqo);
    // Deduplicate within a single record to avoid double-counting.
    let mut seen = std::collections::HashSet::new();
    for name in names {
        if !seen.insert(name.clone()) {
            continue;
        }
        let entry = acc
            .entry(name.clone())
            .or_insert_with(|| EntityAccumulator::new(&name));

        entry.total_interactions += 1;

        if record.quality.first_attempt_bind {
            entry.first_attempt_successes += 1;
        }

        entry.retry_histogram.push(record.quality.bind_attempt_count);

        // Filled = execute returned Success with result_empty == false.
        if let ExecuteOutcome::Success {
            result_empty: false,
            ..
        } = &record.execute_result
        {
            entry.filled_results += 1;
        }

        if let Some(score) = record.grounding_score {
            entry.grounding_scores.push(score);
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_trace_store::{BindOutcome, ExecuteOutcome, QualitySignals, TraceRecord};
    use serde_json::json;

    fn make_record(
        session_id: &str,
        bind_attempt_count: u8,
        first_attempt_bind: bool,
        bind_outcome: BindOutcome,
        execute_result: ExecuteOutcome,
        grounding_score: Option<f64>,
        mqo: serde_json::Value,
    ) -> TraceRecord {
        TraceRecord {
            record_id: "test-id".to_owned(),
            session_id: session_id.to_owned(),
            cluster_name: None,
            timestamp_ms: 1_000,
            mqo,
            bind_outcome,
            grounding_score,
            grounding_band: None,
            execute_result,
            quality: QualitySignals {
                first_attempt_bind,
                bind_attempt_count,
                total_latency_ms: 100,
                tokens_used: None,
            },
            user_question: None,
        }
    }

    #[test]
    fn test_empty_reader_returns_empty_maps() {
        let input = b"";
        let report = score_reader(std::io::Cursor::new(input)).unwrap();
        assert!(report.sessions.is_empty());
        assert!(report.entities.is_empty());
    }

    #[test]
    fn test_session_retry_rate() {
        let records = vec![
            make_record(
                "s1", 2, false, BindOutcome::Success,
                ExecuteOutcome::Success { row_count: 1, result_empty: false },
                None, json!({"measures": [{"unique_name": "revenue"}]}),
            ),
            make_record(
                "s1", 1, true, BindOutcome::Success,
                ExecuteOutcome::Success { row_count: 1, result_empty: false },
                None, json!({"measures": [{"unique_name": "revenue"}]}),
            ),
        ];
        let jsonl: String = records
            .iter()
            .map(|r| serde_json::to_string(r).unwrap() + "\n")
            .collect();
        let report = score_reader(std::io::Cursor::new(jsonl.as_bytes())).unwrap();
        let sq = &report.sessions["s1"];
        // 1 out of 2 retried => 0.5
        assert!((sq.retry_rate - 0.5).abs() < 1e-9);
        assert_eq!(sq.total_mqos, 2);
    }

    #[test]
    fn test_parse_error_carries_line_number() {
        let jsonl = "valid-but-not-trace-record-json\n";
        let err = score_reader(std::io::Cursor::new(jsonl.as_bytes())).unwrap_err();
        match err {
            ScorerError::ParseError { line_number, .. } => assert_eq!(line_number, 1),
            other => panic!("Expected ParseError, got {other:?}"),
        }
    }
}
