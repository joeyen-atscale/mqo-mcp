//! History entry types: the JSONL store schema.
//!
//! # Stability contract
//!
//! Fields on these structs are consumed by `mqo-parity-brief` and CI.
//! Evolution rules:
//! - New fields MAY be added (additive).
//! - Removing, renaming, or re-typing a field is a **breaking change**;
//!   bump `schema_version` on the entry and keep old readers working.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Current schema version carried on every history entry.
pub const SCHEMA_VERSION: u32 = 1;

/// The parsed/normalised verdict for one (measure, backend-pair).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParsedVerdict {
    /// Both backends returned results that agree (Equal or WithinTolerance).
    Verified,
    /// Backends disagree beyond tolerance.
    Mismatch,
    /// This pair was skipped — one or both backends did not execute.
    Skipped,
}

impl ParsedVerdict {
    /// Returns `true` if this verdict contributes evidence (verified or mismatch).
    #[must_use]
    pub fn is_evidence(&self) -> bool {
        matches!(self, Self::Verified | Self::Mismatch)
    }
}

/// One (measure, backend_a, backend_b) parity result within a history entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairRecord {
    /// The measure unique-name.
    pub measure: String,
    /// First backend in the pair.
    pub backend_a: String,
    /// Second backend in the pair.
    pub backend_b: String,
    /// The normalised verdict for this (measure, pair).
    pub verdict: ParsedVerdict,
    /// The raw `PairVerdict` discriminant from the source report (for debugging).
    pub raw_pair_verdict: String,
    /// The `OverallVerdict` of the source report (for context).
    pub raw_overall_verdict: String,
}

impl PairRecord {
    /// Canonical key for de-duplication and coverage lookup.
    #[must_use]
    pub fn key(&self) -> PairKey {
        PairKey {
            measure: self.measure.clone(),
            backend_a: self.backend_a.clone(),
            backend_b: self.backend_b.clone(),
        }
    }
}

/// The canonical (measure, backend_a, backend_b) key used for coverage lookups.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PairKey {
    /// Measure unique-name.
    pub measure: String,
    /// First backend.
    pub backend_a: String,
    /// Second backend.
    pub backend_b: String,
}

/// A single entry in the JSONL history store, one per `record` invocation.
///
/// The `content_digest` field enables idempotent-safe retry detection (FR5):
/// a re-record of the same `(build_id, normalised-pairs)` produces an identical
/// digest, so coverage computation can de-duplicate before counting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Schema version — increment on breaking field changes.
    pub schema_version: u32,
    /// The server build that produced the parity results.
    pub build_id: String,
    /// UTC timestamp at which this entry was ingested (recorded at `record` time,
    /// never recomputed at `coverage` time — NFR1).
    pub ingest_ts: DateTime<Utc>,
    /// Source path or label identifying the input file.
    pub source: String,
    /// The `OverallVerdict` of the source report (stringified).
    pub overall_verdict: String,
    /// Whether every pair in this report was skipped (FR6).
    pub all_skipped: bool,
    /// Per-(measure, pair) records. Empty when `all_skipped` is true.
    pub pairs: Vec<PairRecord>,
    /// SHA-256 of the canonical JSON of `(build_id, sorted pairs)`.
    /// Used for idempotent-safe duplicate detection (FR5).
    pub content_digest: String,
    /// The `parity-corpus.vN` version the caller used, if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corpus_version: Option<String>,
}
