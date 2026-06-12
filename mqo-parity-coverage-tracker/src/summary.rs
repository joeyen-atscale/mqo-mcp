//! Coverage and delta computation.
//!
//! Both functions are **fully deterministic and offline**: given identical
//! `(history, corpus, build_id)` inputs they produce identical output (NFR1).
//! No network calls, no clock reads.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::corpus::Corpus;
use crate::history::{HistoryEntry, PairKey, ParsedVerdict};

// ── Coverage status ────────────────────────────────────────────────────────

/// Per-(measure, backend-pair) coverage status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    /// Latest recorded verdict is `Equal` or `WithinTolerance`.
    Verified,
    /// Latest recorded verdict is `Mismatch`.
    Mismatch,
    /// In the corpus denominator but no non-skipped verdict recorded at this build.
    NeverTested,
}

/// One cell in the coverage grid.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageCell {
    /// The measure unique-name.
    pub measure: String,
    /// First backend.
    pub backend_a: String,
    /// Second backend.
    pub backend_b: String,
    /// The coverage status for this (measure, pair) at the queried build.
    pub status: CoverageStatus,
    /// Whether the corpus marks this case as `has_grounding = false`.
    pub ungrounded: bool,
}

/// Summary returned by [`coverage_summary`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverageSummary {
    /// The build id this summary is for.
    pub build_id: String,
    /// The corpus version used as the denominator.
    pub corpus_version: String,
    /// Number of (measure, pair) cells with status `Verified`.
    pub verified_count: usize,
    /// Number of (measure, pair) cells with status `Mismatch`.
    pub mismatch_count: usize,
    /// Number of (measure, pair) cells with status `NeverTested`.
    pub never_tested_count: usize,
    /// Overall coverage percentage: verified / total corpus cells.
    pub coverage_pct: f64,
    /// Total denominator (all corpus cells for the pair of interest).
    pub denominator: usize,
    /// The backend pair used as the denominator.
    pub backend_a: String,
    /// Second backend.
    pub backend_b: String,
    /// Per-cell detail.
    pub cells: Vec<CoverageCell>,
    /// Cells whose corpus entry has `has_grounding = false` (reported separately per FR10).
    pub ungrounded_cells: Vec<CoverageCell>,
}

// ── Delta types ────────────────────────────────────────────────────────────

/// Summary returned by [`delta_summary`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaSummary {
    /// The candidate (newer) build.
    pub build_n: String,
    /// The prior build.
    pub build_prior: String,
    /// Measures that went from `never-tested`/absent or `mismatch` at prior → `verified` at N.
    pub newly_verified: Vec<String>,
    /// Measures that went from `verified` at prior → `mismatch` at N (regressions).
    pub newly_broken: Vec<String>,
    /// Measures that went from `never-tested`/absent at prior → any non-skipped verdict at N.
    pub newly_tested: Vec<String>,
    /// Whether the prior build had no recorded history (all current verdicts treated as newly-*).
    pub prior_was_absent: bool,
}

// ── Coverage computation ───────────────────────────────────────────────────

/// Compute denominator-based coverage for `build_id` from `history` and `corpus`.
///
/// Uses all (measure, pair) cells declared in the corpus, matching against
/// the history for `build_id`. Backend pair is determined by the backend
/// names present in the history for that build; if the history is empty the
/// caller must supply `backend_a`/`backend_b` via `hint_pair`.
///
/// If `build_id` has no history entries, every cell is `NeverTested` (AC-8).
///
/// Duplicate entries for the same (build_id, measure, pair) are de-duplicated
/// by content_digest first, then latest ingest_ts wins (FR5, FR9).
#[must_use]
pub fn coverage_summary(
    history: &[HistoryEntry],
    corpus: &Corpus,
    build_id: &str,
    hint_pair: Option<(&str, &str)>,
) -> CoverageSummary {
    // Determine backend pair: from history entries for this build, or hint.
    let (backend_a, backend_b) = detect_pair(history, build_id, hint_pair);

    // Build latest-verdict map for this build, de-duplicated by digest then ts.
    let verdict_map = build_verdict_map(history, build_id);

    // Corpus ungrounded set.
    let ungrounded_measures: HashSet<String> = corpus
        .cases
        .iter()
        .filter(|c| !c.has_grounding)
        .flat_map(|c| c.measures.iter().cloned())
        .collect();

    let mut cells = Vec::new();
    let mut ungrounded_cells = Vec::new();
    let mut verified_count = 0usize;
    let mut mismatch_count = 0usize;
    let mut never_tested_count = 0usize;

    let denominator_cells = corpus.cells_for_pair(&backend_a, &backend_b);

    // De-duplicate corpus cells (a measure may appear in multiple cases at different grains).
    // For v1: treat each (measure, pair) combination as one denominator cell.
    let mut seen_cells: HashSet<(String, String, String)> = HashSet::new();
    let mut dedup_denominator = 0usize;

    for (measure, ba, bb) in &denominator_cells {
        let cell_key = (measure.clone(), ba.clone(), bb.clone());
        if !seen_cells.insert(cell_key) {
            continue; // already counted
        }
        dedup_denominator += 1;

        let key = PairKey {
            measure: measure.clone(),
            backend_a: ba.clone(),
            backend_b: bb.clone(),
        };
        let ungrounded = ungrounded_measures.contains(measure);

        let status = match verdict_map.get(&key) {
            Some(ParsedVerdict::Verified) => {
                verified_count += 1;
                CoverageStatus::Verified
            }
            Some(ParsedVerdict::Mismatch) => {
                mismatch_count += 1;
                CoverageStatus::Mismatch
            }
            _ => {
                never_tested_count += 1;
                CoverageStatus::NeverTested
            }
        };

        let cell = CoverageCell {
            measure: measure.clone(),
            backend_a: ba.clone(),
            backend_b: bb.clone(),
            status,
            ungrounded,
        };

        if ungrounded {
            ungrounded_cells.push(cell.clone());
        }
        cells.push(cell);
    }

    let coverage_pct = if dedup_denominator == 0 {
        0.0
    } else {
        (verified_count as f64 / dedup_denominator as f64) * 100.0
    };

    CoverageSummary {
        build_id: build_id.to_string(),
        corpus_version: corpus.version.clone(),
        verified_count,
        mismatch_count,
        never_tested_count,
        coverage_pct,
        denominator: dedup_denominator,
        backend_a,
        backend_b,
        cells,
        ungrounded_cells,
    }
}

// ── Delta computation ──────────────────────────────────────────────────────

/// Compute the build-over-build delta between `build_n` (candidate) and
/// `build_prior` (prior build).
///
/// If `build_prior` has no history, every verdict at `build_n` is treated as
/// newly-tested/newly-verified (AC open-question #4 resolution: warn + proceed).
#[must_use]
pub fn delta_summary(
    history: &[HistoryEntry],
    corpus: &Corpus,
    build_n: &str,
    build_prior: &str,
    hint_pair: Option<(&str, &str)>,
) -> DeltaSummary {
    let (backend_a, backend_b) = detect_pair(history, build_n, hint_pair);

    let map_n = build_verdict_map(history, build_n);
    let map_prior = build_verdict_map(history, build_prior);
    let prior_was_absent = map_prior.is_empty();

    if prior_was_absent {
        eprintln!(
            "WARNING: mqo-parity-coverage-tracker: no history found for prior build '{}'; \
             treating all current verdicts as newly-tested/verified.",
            build_prior
        );
    }

    let all_cells = corpus.cells_for_pair(&backend_a, &backend_b);
    let mut seen: HashSet<(String, String, String)> = HashSet::new();

    let mut newly_verified = Vec::new();
    let mut newly_broken = Vec::new();
    let mut newly_tested = Vec::new();

    for (measure, ba, bb) in &all_cells {
        let cell_key = (measure.clone(), ba.clone(), bb.clone());
        if !seen.insert(cell_key) {
            continue;
        }

        let key = PairKey {
            measure: measure.clone(),
            backend_a: ba.clone(),
            backend_b: bb.clone(),
        };

        let status_n = map_n.get(&key);
        let status_prior = map_prior.get(&key);

        // newly-broken: verified at prior → mismatch at N (FR11)
        if matches!(status_prior, Some(ParsedVerdict::Verified))
            && matches!(status_n, Some(ParsedVerdict::Mismatch))
        {
            newly_broken.push(measure.clone());
        }

        // newly-verified: absent/mismatch at prior → verified at N (FR11)
        if !matches!(status_prior, Some(ParsedVerdict::Verified))
            && matches!(status_n, Some(ParsedVerdict::Verified))
        {
            newly_verified.push(measure.clone());
        }

        // newly-tested: absent at prior → any non-skipped evidence at N (FR11)
        let prior_absent = status_prior.is_none() || prior_was_absent;
        let n_has_evidence = matches!(
            status_n,
            Some(ParsedVerdict::Verified) | Some(ParsedVerdict::Mismatch)
        );
        if prior_absent && n_has_evidence {
            newly_tested.push(measure.clone());
        }
    }

    newly_verified.sort();
    newly_broken.sort();
    newly_tested.sort();

    DeltaSummary {
        build_n: build_n.to_string(),
        build_prior: build_prior.to_string(),
        newly_verified,
        newly_broken,
        newly_tested,
        prior_was_absent,
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Build a `{ PairKey => latest ParsedVerdict }` map for the given build.
///
/// De-duplicates by content_digest first (FR5), then takes the latest
/// ingest_ts for any remaining duplicates (FR9).
fn build_verdict_map(history: &[HistoryEntry], build_id: &str) -> HashMap<PairKey, ParsedVerdict> {
    // Collect entries for this build, de-duplicated by content_digest.
    let mut seen_digests: HashSet<String> = HashSet::new();
    let mut build_entries: Vec<&HistoryEntry> = history
        .iter()
        .filter(|e| e.build_id == build_id)
        .filter(|e| {
            if seen_digests.contains(&e.content_digest) {
                false
            } else {
                seen_digests.insert(e.content_digest.clone());
                true
            }
        })
        .collect();

    // Sort by ingest_ts ascending so later entries win.
    build_entries.sort_by_key(|e| e.ingest_ts);

    let mut map: HashMap<PairKey, ParsedVerdict> = HashMap::new();
    for entry in build_entries {
        if entry.all_skipped {
            // AllSkipped entries contribute no coverage evidence (FR6, AC-5).
            continue;
        }
        for pair in &entry.pairs {
            if pair.verdict.is_evidence() {
                map.insert(pair.key(), pair.verdict.clone());
            }
        }
    }
    map
}

/// Detect the backend pair to use for the denominator.
///
/// Prefers the pair found in history entries for `build_id`; falls back to
/// `hint_pair`; defaults to ("dax", "sql").
fn detect_pair(
    history: &[HistoryEntry],
    build_id: &str,
    hint: Option<(&str, &str)>,
) -> (String, String) {
    for entry in history {
        if entry.build_id == build_id {
            for pair in &entry.pairs {
                return (pair.backend_a.clone(), pair.backend_b.clone());
            }
        }
    }
    if let Some((a, b)) = hint {
        return (a.to_string(), b.to_string());
    }
    ("dax".to_string(), "sql".to_string())
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::{Corpus, CorpusCase};
    use crate::history::{HistoryEntry, PairRecord, ParsedVerdict, SCHEMA_VERSION};
    use chrono::Utc;

    fn make_corpus(measures: &[&str]) -> Corpus {
        Corpus {
            version: "parity-corpus.v1".to_string(),
            catalog: "test-catalog".to_string(),
            cases: measures
                .iter()
                .map(|m| CorpusCase {
                    case_id: format!("{}_total", m),
                    measures: vec![m.to_string()],
                    grain: "total".to_string(),
                    has_grounding: true,
                })
                .collect(),
        }
    }

    fn make_entry(build_id: &str, measure: &str, verdict: ParsedVerdict) -> HistoryEntry {
        let pairs = vec![PairRecord {
            measure: measure.to_string(),
            backend_a: "dax".to_string(),
            backend_b: "sql".to_string(),
            verdict,
            raw_pair_verdict: "Equal".to_string(),
            raw_overall_verdict: "Agree".to_string(),
        }];
        HistoryEntry {
            schema_version: SCHEMA_VERSION,
            build_id: build_id.to_string(),
            ingest_ts: Utc::now(),
            source: "test".to_string(),
            overall_verdict: "Agree".to_string(),
            all_skipped: false,
            content_digest: format!("digest-{}-{}", build_id, measure),
            pairs,
            corpus_version: None,
        }
    }

    fn make_all_skipped_entry(build_id: &str) -> HistoryEntry {
        HistoryEntry {
            schema_version: SCHEMA_VERSION,
            build_id: build_id.to_string(),
            ingest_ts: Utc::now(),
            source: "test-skipped".to_string(),
            overall_verdict: "AllSkipped".to_string(),
            all_skipped: true,
            content_digest: format!("digest-skipped-{}", build_id),
            pairs: vec![],
            corpus_version: None,
        }
    }

    // AC-2: verified 1/3 measures → coverage 33.3%
    #[test]
    fn test_coverage_one_of_three_verified() {
        let corpus = make_corpus(&["Total Store Sales", "Margin %", "Returns Amount"]);
        let history = vec![make_entry("build-B", "Total Store Sales", ParsedVerdict::Verified)];
        let summary = coverage_summary(&history, &corpus, "build-B", None);
        assert_eq!(summary.verified_count, 1);
        assert_eq!(summary.never_tested_count, 2);
        assert_eq!(summary.mismatch_count, 0);
        assert_eq!(summary.denominator, 3);
        let pct = (1.0_f64 / 3.0) * 100.0;
        assert!((summary.coverage_pct - pct).abs() < 0.01);
        let tss = summary.cells.iter().find(|c| c.measure == "Total Store Sales").unwrap();
        assert_eq!(tss.status, CoverageStatus::Verified);
        let returns = summary.cells.iter().find(|c| c.measure == "Returns Amount").unwrap();
        assert_eq!(returns.status, CoverageStatus::NeverTested);
    }

    // AC-4: measure in corpus, no result → never-tested
    #[test]
    fn test_coverage_no_result_is_never_tested() {
        let corpus = make_corpus(&["Returns Amount"]);
        let history: Vec<HistoryEntry> = vec![];
        let summary = coverage_summary(&history, &corpus, "build-X", None);
        assert_eq!(summary.never_tested_count, 1);
        assert_eq!(summary.verified_count, 0);
        assert!((summary.coverage_pct - 0.0).abs() < 0.01);
    }

    // AC-5: AllSkipped result doesn't change coverage
    #[test]
    fn test_all_skipped_does_not_change_coverage() {
        let corpus = make_corpus(&["Total Store Sales"]);
        let baseline = vec![make_entry("build-B", "Total Store Sales", ParsedVerdict::Verified)];

        let with_skipped = {
            let mut h = baseline.clone();
            h.push(make_all_skipped_entry("build-B"));
            h
        };

        let summary_base = coverage_summary(&baseline, &corpus, "build-B", None);
        let summary_with = coverage_summary(&with_skipped, &corpus, "build-B", None);

        assert_eq!(summary_base.verified_count, summary_with.verified_count);
        assert!((summary_base.coverage_pct - summary_with.coverage_pct).abs() < 0.01);
    }

    // AC-8: empty history → all never-tested, 0%, exit 0
    #[test]
    fn test_empty_history_all_never_tested() {
        let corpus = make_corpus(&["Measure A", "Measure B"]);
        let summary = coverage_summary(&[], &corpus, "build-unknown", None);
        assert_eq!(summary.never_tested_count, 2);
        assert_eq!(summary.verified_count, 0);
        assert!((summary.coverage_pct - 0.0).abs() < 0.01);
    }

    // AC-3: Margin % Equal at N-1, Mismatch at N → newly-broken
    #[test]
    fn test_delta_newly_broken() {
        let corpus = make_corpus(&["Margin %"]);
        let history = vec![
            make_entry("build-N-1", "Margin %", ParsedVerdict::Verified),
            make_entry("build-N", "Margin %", ParsedVerdict::Mismatch),
        ];
        let delta = delta_summary(&history, &corpus, "build-N", "build-N-1", None);
        assert!(delta.newly_broken.contains(&"Margin %".to_string()));
        assert!(!delta.newly_verified.contains(&"Margin %".to_string()));
    }

    // AC-3 supplement: newly-verified appears in newly_verified
    #[test]
    fn test_delta_newly_verified() {
        let corpus = make_corpus(&["Returns Amount"]);
        let history = vec![
            make_entry("build-N-1", "Returns Amount", ParsedVerdict::Mismatch),
            make_entry("build-N", "Returns Amount", ParsedVerdict::Verified),
        ];
        let delta = delta_summary(&history, &corpus, "build-N", "build-N-1", None);
        assert!(delta.newly_verified.contains(&"Returns Amount".to_string()));
        assert!(!delta.newly_broken.contains(&"Returns Amount".to_string()));
    }

    // Delta: prior build absent → all newly-tested
    #[test]
    fn test_delta_absent_prior_all_newly_tested() {
        let corpus = make_corpus(&["Measure Z"]);
        let history = vec![make_entry("build-N", "Measure Z", ParsedVerdict::Verified)];
        let delta = delta_summary(&history, &corpus, "build-N", "build-does-not-exist", None);
        assert!(delta.prior_was_absent);
        assert!(delta.newly_tested.contains(&"Measure Z".to_string()));
        assert!(delta.newly_verified.contains(&"Measure Z".to_string()));
    }
}
