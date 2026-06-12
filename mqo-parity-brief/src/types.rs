// types.rs — data structures matching the mqo-parity-coverage-tracker JSONL history store.
// Status vocabulary and delta vocabulary are used verbatim per NFR3.

use serde::{Deserialize, Serialize};

/// A single entry in the parity coverage history store (one JSONL line per build pass).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct HistoryRecord {
    /// Unique build identifier (e.g. "b-2026-06-10.1").  Must never be "latest" or an alias.
    pub build_id: String,

    /// Human-readable version string (e.g. "v0.3.0").
    pub version: String,

    /// Cluster hostname where the parity pass was run.
    pub cluster: String,

    /// ISO-8601 timestamp of when this record was appended.
    pub recorded_at: String,

    /// The tracker's overall verdict for this build pass.
    /// Possible values: "Agree" | "WithinTolerance" | "Mismatch" | "AllSkipped"
    pub overall_verdict: String,

    /// Per-(measure, backend-pair) statuses for this build.
    pub measures: Vec<MeasureStatus>,

    /// Deltas vs the immediately prior build.  Absent when this is the first build.
    #[serde(default)]
    pub deltas: Deltas,
}

/// Status vocabulary per NFR3: exactly these three strings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Verified,
    Mismatch,
    #[serde(rename = "never-tested")]
    NeverTested,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Status::Verified => write!(f, "verified"),
            Status::Mismatch => write!(f, "mismatch"),
            Status::NeverTested => write!(f, "never-tested"),
        }
    }
}

/// Status of a single (measure, backend-pair) slot in a build.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MeasureStatus {
    pub measure: String,
    pub backend_pair: String,
    pub status: Status,
}

/// Build-over-build deltas.  Delta vocabulary per NFR3.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Deltas {
    /// Measures that newly disagree this build vs the prior build.
    #[serde(default)]
    pub newly_broken: Vec<DeltaEntry>,

    /// Measures that newly agree this build vs the prior build (recoveries).
    #[serde(default)]
    pub newly_verified: Vec<DeltaEntry>,
}

/// A single entry in a delta list.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeltaEntry {
    pub measure: String,
    pub backend_pair: String,
}

/// Tiger-compatible build-stamped record (FR7 / G5).
/// Shape is intentionally minimal pending OQ2 (Tiger record contract).
#[derive(Debug, Serialize)]
pub struct TigerRecord {
    pub build_id: String,
    pub version: String,
    pub cluster: String,
    pub parity_coverage_pct: Option<f64>,
    /// Per-backend-pair counts as (pair, verified, mismatch, never_tested).
    pub backend_pair_counts: Vec<PairCounts>,
}

#[derive(Debug, Serialize)]
pub struct PairCounts {
    pub backend_pair: String,
    pub verified: usize,
    pub mismatch: usize,
    pub never_tested: usize,
}
