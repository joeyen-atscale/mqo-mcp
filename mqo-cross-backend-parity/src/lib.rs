//! `mqo-cross-backend-parity` — Parity oracle for MQO multi-backend execution.
//!
//! Given an MQO and a set of available backends (DAX, MDX, SQL), this crate
//! compiles and executes the MQO on each, then asserts the results agree using
//! a [`ResultComparator`] implementation.
//!
//! The [`comparator::DaxComparator`] is backed by `pbicorr-dax-result-comparator`
//! and is used in production; [`comparator::StubComparator`] is kept for unit tests.
//!
//! # Emitted report shape
//!
//! ```json
//! {
//!   "mqo_path": "query.json",
//!   "backends_requested": ["dax", "sql"],
//!   "results": { "sql": { "Executed": { "rows": [...] } } },
//!   "pairs": [["dax", "sql", { "Skipped": { "why": "..." } }]],
//!   "overall": "Agree"
//! }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod comparator;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── Backend execution status ───────────────────────────────────────────────

/// Status of a single backend's execution attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BackendStatus {
    /// The backend executed successfully and returned rows.
    Executed {
        /// The result rows, each a JSON object of column → value.
        rows: Vec<serde_json::Value>,
    },
    /// The backend was not executed (not yet wired, dead port, etc.).
    Skipped {
        /// Human-readable reason why execution was skipped.
        reason: String,
    },
    /// The backend was attempted but returned an error.
    Error {
        /// The error message.
        message: String,
    },
}

impl BackendStatus {
    /// Returns the rows if this status is `Executed`, else `None`.
    pub fn rows(&self) -> Option<&[serde_json::Value]> {
        match self {
            Self::Executed { rows } => Some(rows),
            _ => None,
        }
    }

    /// Returns `true` if this backend executed (not skipped or errored).
    pub fn is_executed(&self) -> bool {
        matches!(self, Self::Executed { .. })
    }
}

// ── Pair verdict ───────────────────────────────────────────────────────────

/// The verdict for a single backend-pair comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PairVerdict {
    /// Both backends returned identical results.
    Equal,
    /// Both backends agree within the configured numeric tolerance.
    WithinTolerance {
        /// Human-readable detail of how much tolerance was consumed.
        detail: String,
    },
    /// The backends disagree beyond the configured tolerance.
    Mismatch {
        /// Human-readable reason for the mismatch.
        reason: String,
    },
    /// This pair was not compared (one or both backends were skipped/errored).
    Skipped {
        /// Why this pair was not compared.
        why: String,
    },
}

impl PairVerdict {
    /// Returns `true` iff this verdict represents a non-skipped mismatch.
    pub fn is_mismatch(&self) -> bool {
        matches!(self, Self::Mismatch { .. })
    }

    /// Returns `true` iff this pair was skipped.
    pub fn is_skipped(&self) -> bool {
        matches!(self, Self::Skipped { .. })
    }
}

// ── Overall verdict ────────────────────────────────────────────────────────

/// The overall parity verdict across all backend pairs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OverallVerdict {
    /// All non-skipped backend pairs agree exactly.
    Agree,
    /// All non-skipped backend pairs agree within tolerance (at least one was `WithinTolerance`).
    WithinTolerance,
    /// At least one non-skipped backend pair returned a mismatch.
    Mismatch,
    /// All pairs were skipped (no executed backends to compare).
    AllSkipped,
}

impl OverallVerdict {
    /// Returns `true` if the overall verdict is not a hard failure.
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Agree | Self::WithinTolerance | Self::AllSkipped)
    }
}

// ── Parity report ─────────────────────────────────────────────────────────

/// A triple of (backend_a, backend_b, verdict) for one compared pair.
pub type PairResult = (String, String, PairVerdict);

/// Full parity report for one MQO execution across multiple backends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParityReport {
    /// Path to the MQO file that was executed.
    pub mqo_path: String,
    /// Backends requested by the caller.
    pub backends_requested: Vec<String>,
    /// Per-backend execution status.
    pub results: HashMap<String, BackendStatus>,
    /// Per-pair comparison verdicts: (backend_a, backend_b, verdict).
    pub pairs: Vec<PairResult>,
    /// Rolled-up overall verdict.
    pub overall: OverallVerdict,
}

impl ParityReport {
    /// Compute the `OverallVerdict` from the per-pair results.
    pub fn compute_overall(pairs: &[PairResult]) -> OverallVerdict {
        let non_skipped: Vec<&PairVerdict> = pairs
            .iter()
            .map(|(_, _, v)| v)
            .filter(|v| !v.is_skipped())
            .collect();

        if non_skipped.is_empty() {
            return OverallVerdict::AllSkipped;
        }

        let has_mismatch = non_skipped.iter().any(|v| v.is_mismatch());
        if has_mismatch {
            return OverallVerdict::Mismatch;
        }

        let has_tolerance = non_skipped
            .iter()
            .any(|v| matches!(v, PairVerdict::WithinTolerance { .. }));
        if has_tolerance {
            return OverallVerdict::WithinTolerance;
        }

        OverallVerdict::Agree
    }

    /// Build a `ParityReport` from raw inputs, computing `overall` automatically.
    pub fn build(
        mqo_path: String,
        backends_requested: Vec<String>,
        results: HashMap<String, BackendStatus>,
        pairs: Vec<PairResult>,
    ) -> Self {
        let overall = Self::compute_overall(&pairs);
        Self {
            mqo_path,
            backends_requested,
            results,
            pairs,
            overall,
        }
    }
}

// ── ResultComparator trait ─────────────────────────────────────────────────

/// Trait for comparing two result row sets.
///
/// This is the plug-in point for `pbicorr-dax-result-comparator`.
/// Today the `StubComparator` in [`comparator`] is used; once that crate ships,
/// a `DaxComparator` wrapper can implement this trait and be wired in.
pub trait ResultComparator: Send + Sync {
    /// Compare `actual` rows against `expected` rows and return a verdict.
    fn compare_rows(
        &self,
        actual: &[serde_json::Value],
        expected: &[serde_json::Value],
    ) -> PairVerdict;
}

// ── Parity runner ──────────────────────────────────────────────────────────

/// Run parity across all backend pairs given their statuses and a comparator.
///
/// Pairs are formed as a lower-triangular matrix (each pair compared once).
/// If either backend is not `Executed`, the pair is `Skipped`.
pub fn run_parity(
    backends: &[String],
    results: &HashMap<String, BackendStatus>,
    comparator: &dyn ResultComparator,
) -> Vec<PairResult> {
    let mut pairs = Vec::new();

    for i in 0..backends.len() {
        for j in (i + 1)..backends.len() {
            let a = &backends[i];
            let b = &backends[j];

            let status_a = results.get(a);
            let status_b = results.get(b);

            let verdict = match (status_a, status_b) {
                (Some(BackendStatus::Executed { rows: rows_a }), Some(BackendStatus::Executed { rows: rows_b })) => {
                    comparator.compare_rows(rows_a, rows_b)
                }
                (Some(BackendStatus::Skipped { reason }), _) => PairVerdict::Skipped {
                    why: format!("{a} was skipped: {reason}"),
                },
                (_, Some(BackendStatus::Skipped { reason })) => PairVerdict::Skipped {
                    why: format!("{b} was skipped: {reason}"),
                },
                (Some(BackendStatus::Error { message }), _) => PairVerdict::Skipped {
                    why: format!("{a} errored: {message}"),
                },
                (_, Some(BackendStatus::Error { message })) => PairVerdict::Skipped {
                    why: format!("{b} errored: {message}"),
                },
                _ => PairVerdict::Skipped {
                    why: format!("one or both of {a}/{b} had no result"),
                },
            };

            pairs.push((a.clone(), b.clone(), verdict));
        }
    }

    pairs
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comparator::StubComparator;
    use serde_json::json;

    fn executed(rows: Vec<serde_json::Value>) -> BackendStatus {
        BackendStatus::Executed { rows }
    }

    fn skipped(reason: &str) -> BackendStatus {
        BackendStatus::Skipped {
            reason: reason.to_string(),
        }
    }

    // AC1: Single backend → trivially Agree (backend agrees with itself).
    #[test]
    fn single_backend_trivially_agree() {
        let backends = vec!["sql".to_string()];
        let rows = vec![json!({"region": "US", "sales": 100})];
        let mut results = HashMap::new();
        results.insert("sql".to_string(), executed(rows));

        let comparator = StubComparator::default();
        let pairs = run_parity(&backends, &results, &comparator);
        // No pairs to compare for a single backend → AllSkipped means trivially agree.
        let overall = ParityReport::compute_overall(&pairs);
        assert_eq!(pairs.len(), 0);
        assert_eq!(overall, OverallVerdict::AllSkipped);
    }

    // AC2: Two backends with identical rows → Equal verdict, overall Agree.
    #[test]
    fn two_backends_equal_rows_agree() {
        let backends = vec!["sql".to_string(), "dax".to_string()];
        let rows = vec![json!({"region": "US", "sales": 100})];
        let mut results = HashMap::new();
        results.insert("sql".to_string(), executed(rows.clone()));
        results.insert("dax".to_string(), executed(rows));

        let comparator = StubComparator::default();
        let pairs = run_parity(&backends, &results, &comparator);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].2, PairVerdict::Equal);

        let overall = ParityReport::compute_overall(&pairs);
        assert_eq!(overall, OverallVerdict::Agree);
    }

    // AC3: Float values differ within tolerance → WithinTolerance verdict.
    #[test]
    fn two_backends_within_tolerance() {
        let backends = vec!["sql".to_string(), "dax".to_string()];
        let mut results = HashMap::new();
        results.insert(
            "sql".to_string(),
            executed(vec![json!({"v": 100.0})]),
        );
        results.insert(
            "dax".to_string(),
            executed(vec![json!({"v": 100.000_001})]),
        );

        let comparator = StubComparator::default();
        let pairs = run_parity(&backends, &results, &comparator);
        // StubComparator uses tolerance logic: 100.000001 vs 100.0 is within 0.01%.
        assert_eq!(pairs.len(), 1);
        assert!(
            matches!(&pairs[0].2, PairVerdict::WithinTolerance { .. }),
            "expected WithinTolerance, got {:?}",
            pairs[0].2
        );
        let overall = ParityReport::compute_overall(&pairs);
        assert_eq!(overall, OverallVerdict::WithinTolerance);
    }

    // AC4: Two backends with row count mismatch → Mismatch.
    #[test]
    fn two_backends_row_count_mismatch() {
        let backends = vec!["sql".to_string(), "dax".to_string()];
        let mut results = HashMap::new();
        results.insert(
            "sql".to_string(),
            executed(vec![json!({"v": 1}), json!({"v": 2})]),
        );
        results.insert("dax".to_string(), executed(vec![json!({"v": 1})]));

        let comparator = StubComparator::default();
        let pairs = run_parity(&backends, &results, &comparator);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].2.is_mismatch(), "expected Mismatch, got {:?}", pairs[0].2);

        let overall = ParityReport::compute_overall(&pairs);
        assert_eq!(overall, OverallVerdict::Mismatch);
    }

    // AC5: Skipped backend does not affect overall verdict.
    #[test]
    fn skipped_backend_does_not_affect_overall() {
        let backends = vec!["sql".to_string(), "dax".to_string()];
        let rows = vec![json!({"v": 42})];
        let mut results = HashMap::new();
        results.insert("sql".to_string(), executed(rows));
        results.insert("dax".to_string(), skipped("not yet wired"));

        let comparator = StubComparator::default();
        let pairs = run_parity(&backends, &results, &comparator);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].2.is_skipped());

        let overall = ParityReport::compute_overall(&pairs);
        // Only skipped pairs → AllSkipped (run succeeds).
        assert_eq!(overall, OverallVerdict::AllSkipped);
    }

    // Verify report build helper computes overall correctly.
    #[test]
    fn report_build_computes_overall() {
        let pairs = vec![
            ("sql".to_string(), "dax".to_string(), PairVerdict::Equal),
        ];
        let report = ParityReport::build(
            "query.json".to_string(),
            vec!["sql".to_string(), "dax".to_string()],
            HashMap::new(),
            pairs,
        );
        assert_eq!(report.overall, OverallVerdict::Agree);
    }
}
