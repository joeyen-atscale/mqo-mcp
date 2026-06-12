//! JSONL history store: `record_run` (append) and `load_history` (read).
//!
//! The store is append-only: `record_run` never modifies prior lines.
//! `load_history` is tolerant of a truncated/corrupt final line (AC-7, NFR2):
//! it skips the bad line, emits a warning to stderr, and returns all valid entries.

use std::io::{BufRead, Write};
use std::path::Path;

use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::history::{HistoryEntry, SCHEMA_VERSION};

// ── Input shape types ──────────────────────────────────────────────────────

/// The two accepted input shapes (FR2).
#[derive(Debug)]
pub enum InputReport {
    /// A `ParityReport` from `mqo-cross-backend-parity`.
    ParityReport(ParityReportInput),
    /// Per-case results from `mqo-backend-live-harness`.
    LiveHarnessResults(Vec<LiveHarnessCase>),
}

/// Mirrors the `ParityReport` shape from `mqo-cross-backend-parity`.
#[derive(Debug, serde::Deserialize)]
pub struct ParityReportInput {
    /// Path/label of the MQO that was executed.
    pub mqo_path: String,
    /// Per-pair comparison results: (backend_a, backend_b, verdict).
    pub pairs: Vec<(String, String, PairVerdictInput)>,
    /// Rolled-up overall verdict.
    pub overall: OverallVerdictInput,
    /// Optional: measure name for this MQO (if the report carries it).
    #[serde(default)]
    pub measure: Option<String>,
}

/// Mirrors `PairVerdict` from `mqo-cross-backend-parity`.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum PairVerdictInput {
    /// Results identical.
    Equal,
    /// Results agree within tolerance.
    WithinTolerance {
        /// Detail string.
        detail: String,
    },
    /// Results disagree.
    Mismatch {
        /// Reason string.
        reason: String,
    },
    /// Pair not compared.
    Skipped {
        /// Why.
        why: String,
    },
}

impl PairVerdictInput {
    fn discriminant(&self) -> &'static str {
        match self {
            Self::Equal => "Equal",
            Self::WithinTolerance { .. } => "WithinTolerance",
            Self::Mismatch { .. } => "Mismatch",
            Self::Skipped { .. } => "Skipped",
        }
    }

    fn to_parsed(&self) -> crate::history::ParsedVerdict {
        match self {
            Self::Equal | Self::WithinTolerance { .. } => crate::history::ParsedVerdict::Verified,
            Self::Mismatch { .. } => crate::history::ParsedVerdict::Mismatch,
            Self::Skipped { .. } => crate::history::ParsedVerdict::Skipped,
        }
    }
}

/// Mirrors `OverallVerdict` from `mqo-cross-backend-parity`.
#[derive(Debug, serde::Deserialize)]
pub enum OverallVerdictInput {
    /// All pairs agree.
    Agree,
    /// All pairs agree within tolerance.
    WithinTolerance,
    /// At least one pair mismatches.
    Mismatch,
    /// All pairs were skipped.
    AllSkipped,
}

impl OverallVerdictInput {
    fn discriminant(&self) -> &'static str {
        match self {
            Self::Agree => "Agree",
            Self::WithinTolerance => "WithinTolerance",
            Self::Mismatch => "Mismatch",
            Self::AllSkipped => "AllSkipped",
        }
    }

    fn is_all_skipped(&self) -> bool {
        matches!(self, Self::AllSkipped)
    }
}

/// One per-case result from `mqo-backend-live-harness`.
#[derive(Debug, serde::Deserialize)]
pub struct LiveHarnessCase {
    /// The measure unique-name.
    pub measure: String,
    /// First backend in the pair.
    pub backend_a: String,
    /// Second backend in the pair.
    pub backend_b: String,
    /// The verdict for this pair.
    pub verdict: PairVerdictInput,
    /// The overall verdict for this case.
    pub overall: OverallVerdictInput,
}

// ── Normalisation ──────────────────────────────────────────────────────────

fn normalise_parity_report(
    report: &ParityReportInput,
    build_id: &str,
    source: &str,
) -> HistoryEntry {
    let all_skipped = report.overall.is_all_skipped();
    let pairs = if all_skipped {
        vec![]
    } else {
        report
            .pairs
            .iter()
            .map(|(a, b, v)| crate::history::PairRecord {
                measure: report
                    .measure
                    .clone()
                    .unwrap_or_else(|| report.mqo_path.clone()),
                backend_a: a.clone(),
                backend_b: b.clone(),
                verdict: v.to_parsed(),
                raw_pair_verdict: v.discriminant().to_string(),
                raw_overall_verdict: report.overall.discriminant().to_string(),
            })
            .filter(|p| !p.verdict.eq(&crate::history::ParsedVerdict::Skipped) || !all_skipped)
            .collect()
    };

    let digest = compute_digest(build_id, &pairs);

    HistoryEntry {
        schema_version: SCHEMA_VERSION,
        build_id: build_id.to_string(),
        ingest_ts: Utc::now(),
        source: source.to_string(),
        overall_verdict: report.overall.discriminant().to_string(),
        all_skipped,
        pairs,
        content_digest: digest,
        corpus_version: None,
    }
}

fn normalise_live_harness(
    cases: &[LiveHarnessCase],
    build_id: &str,
    source: &str,
) -> HistoryEntry {
    let all_skipped = cases.iter().all(|c| c.overall.is_all_skipped());

    let pairs: Vec<crate::history::PairRecord> = if all_skipped {
        vec![]
    } else {
        cases
            .iter()
            .map(|c| crate::history::PairRecord {
                measure: c.measure.clone(),
                backend_a: c.backend_a.clone(),
                backend_b: c.backend_b.clone(),
                verdict: c.verdict.to_parsed(),
                raw_pair_verdict: c.verdict.discriminant().to_string(),
                raw_overall_verdict: c.overall.discriminant().to_string(),
            })
            .collect()
    };

    let overall = if all_skipped {
        "AllSkipped"
    } else if cases.iter().any(|c| matches!(c.overall, OverallVerdictInput::Mismatch)) {
        "Mismatch"
    } else {
        "Agree"
    };

    let digest = compute_digest(build_id, &pairs);

    HistoryEntry {
        schema_version: SCHEMA_VERSION,
        build_id: build_id.to_string(),
        ingest_ts: Utc::now(),
        source: source.to_string(),
        overall_verdict: overall.to_string(),
        all_skipped,
        pairs,
        content_digest: digest,
        corpus_version: None,
    }
}

// ── Digest (FR5) ───────────────────────────────────────────────────────────

fn compute_digest(build_id: &str, pairs: &[crate::history::PairRecord]) -> String {
    // Sort pairs deterministically so digest is stable regardless of insertion order.
    let mut sorted: Vec<_> = pairs.iter().collect();
    sorted.sort_by_key(|p| (&p.measure, &p.backend_a, &p.backend_b));

    #[derive(serde::Serialize)]
    struct DigestPayload<'a> {
        build_id: &'a str,
        pairs: Vec<DigestPair<'a>>,
    }
    #[derive(serde::Serialize)]
    struct DigestPair<'a> {
        measure: &'a str,
        backend_a: &'a str,
        backend_b: &'a str,
        verdict: &'a crate::history::ParsedVerdict,
    }

    let payload = DigestPayload {
        build_id,
        pairs: sorted
            .iter()
            .map(|p| DigestPair {
                measure: &p.measure,
                backend_a: &p.backend_a,
                backend_b: &p.backend_b,
                verdict: &p.verdict,
            })
            .collect(),
    };

    let canonical = serde_json::to_string(&payload).unwrap_or_default();
    let hash = Sha256::digest(canonical.as_bytes());
    hex::encode(hash)
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Errors from store operations.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// An I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// A JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Normalise an [`InputReport`] and append it to the JSONL store at `store_path`.
///
/// Creates the store file (and parent directories) if they do not exist.
/// Never modifies prior lines (FR3).
///
/// # Errors
///
/// Returns [`StoreError`] on I/O or JSON serialization failure.
pub fn record_run(
    input: &InputReport,
    build_id: &str,
    source: &str,
    store_path: &Path,
) -> Result<HistoryEntry, StoreError> {
    let entry = match input {
        InputReport::ParityReport(r) => normalise_parity_report(r, build_id, source),
        InputReport::LiveHarnessResults(cases) => normalise_live_harness(cases, build_id, source),
    };

    if let Some(parent) = store_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(store_path)?;

    let line = serde_json::to_string(&entry)?;
    writeln!(file, "{}", line)?;

    Ok(entry)
}

/// Load all valid entries from the JSONL store at `store_path`.
///
/// Tolerates a missing file (returns empty vec) and corrupt/partial lines
/// (skips them, emits a warning to stderr — AC-7, NFR2).
///
/// # Errors
///
/// Returns [`StoreError`] only on an I/O error opening the file (not on
/// per-line parse failures, which are skipped with a warning).
pub fn load_history(store_path: &Path) -> Result<Vec<HistoryEntry>, StoreError> {
    if !store_path.exists() {
        return Ok(vec![]);
    }

    let file = std::fs::File::open(store_path)?;
    let reader = std::io::BufReader::new(file);
    let mut entries = Vec::new();

    for (line_no, line_result) in reader.lines().enumerate() {
        let line = line_result?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<HistoryEntry>(trimmed) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                eprintln!(
                    "WARNING: mqo-parity-coverage-tracker: skipping corrupt line {} in {}: {}",
                    line_no + 1,
                    store_path.display(),
                    e
                );
            }
        }
    }

    Ok(entries)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn make_parity_report(measure: &str, verdict: PairVerdictInput, overall: OverallVerdictInput) -> InputReport {
        InputReport::ParityReport(ParityReportInput {
            mqo_path: format!("{}.json", measure),
            pairs: vec![("dax".to_string(), "sql".to_string(), verdict)],
            overall,
            measure: Some(measure.to_string()),
        })
    }

    #[test]
    fn test_record_appends_exactly_one_line() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path();

        let input = make_parity_report("Total Store Sales", PairVerdictInput::Equal, OverallVerdictInput::Agree);
        record_run(&input, "2026.06.10-rc1", "test", path).unwrap();

        let entries = load_history(path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].build_id, "2026.06.10-rc1");
    }

    #[test]
    fn test_record_preserves_prior_lines() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path();

        let input1 = make_parity_report("Measure A", PairVerdictInput::Equal, OverallVerdictInput::Agree);
        record_run(&input1, "build-1", "test", path).unwrap();

        let input2 = make_parity_report("Measure B", PairVerdictInput::Mismatch { reason: "diff".to_string() }, OverallVerdictInput::Mismatch);
        record_run(&input2, "build-2", "test", path).unwrap();

        let entries = load_history(path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].build_id, "build-1");
        assert_eq!(entries[1].build_id, "build-2");
    }

    #[test]
    fn test_all_skipped_result_recorded_but_no_pairs() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path();

        let input = make_parity_report(
            "Skipped Measure",
            PairVerdictInput::Skipped { why: "backend down".to_string() },
            OverallVerdictInput::AllSkipped,
        );
        record_run(&input, "build-skip", "test", path).unwrap();

        let entries = load_history(path).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].all_skipped);
        assert!(entries[0].pairs.is_empty());
    }

    #[test]
    fn test_load_history_empty_file() {
        let file = NamedTempFile::new().unwrap();
        let entries = load_history(file.path()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_load_history_missing_file() {
        let path = Path::new("/tmp/mqo-parity-test-does-not-exist-1234567890.jsonl");
        let entries = load_history(path).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_load_history_corrupt_trailing_line() {
        use std::io::Write;
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();

        // Write one valid entry
        let input = make_parity_report("Total Store Sales", PairVerdictInput::Equal, OverallVerdictInput::Agree);
        record_run(&input, "build-1", "test", &path).unwrap();

        // Append a corrupt line
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{{\"corrupt\": true, incomplete_json").unwrap();

        let entries = load_history(&path).unwrap();
        assert_eq!(entries.len(), 1, "corrupt line should be skipped");
        assert_eq!(entries[0].build_id, "build-1");
    }

    #[test]
    fn test_content_digest_is_stable() {
        let file1 = NamedTempFile::new().unwrap();
        let file2 = NamedTempFile::new().unwrap();

        let input1 = make_parity_report("Measure X", PairVerdictInput::Equal, OverallVerdictInput::Agree);
        let input2 = make_parity_report("Measure X", PairVerdictInput::Equal, OverallVerdictInput::Agree);

        let e1 = record_run(&input1, "build-1", "src1", file1.path()).unwrap();
        let e2 = record_run(&input2, "build-1", "src2", file2.path()).unwrap();

        // Same (build_id, normalised-pairs) => same digest regardless of source label
        assert_eq!(e1.content_digest, e2.content_digest);
    }

    #[test]
    fn test_live_harness_normalisation() {
        let file = NamedTempFile::new().unwrap();
        let path = file.path();

        let cases = vec![
            LiveHarnessCase {
                measure: "Margin %".to_string(),
                backend_a: "dax".to_string(),
                backend_b: "sql".to_string(),
                verdict: PairVerdictInput::Equal,
                overall: OverallVerdictInput::Agree,
            },
            LiveHarnessCase {
                measure: "Returns Amount".to_string(),
                backend_a: "dax".to_string(),
                backend_b: "sql".to_string(),
                verdict: PairVerdictInput::Mismatch { reason: "numeric diff".to_string() },
                overall: OverallVerdictInput::Mismatch,
            },
        ];

        let input = InputReport::LiveHarnessResults(cases);
        let entry = record_run(&input, "build-lh", "harness.json", path).unwrap();

        assert_eq!(entry.pairs.len(), 2);
        assert_eq!(entry.pairs[0].verdict, crate::history::ParsedVerdict::Verified);
        assert_eq!(entry.pairs[1].verdict, crate::history::ParsedVerdict::Mismatch);
    }
}
