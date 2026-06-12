//! mqo-paramq-bench — offline pass@k harness for free-form vs structured MQO.
//!
//! Pure library; the binary thin-wrapper lives in main.rs.

use mqo_param_validator::CatalogSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Corpus types
// ---------------------------------------------------------------------------

/// A task from the tpcds_failure_modes corpus.
#[derive(Debug, Clone, Deserialize)]
pub struct CorpusTask {
    pub id: String,
    pub failure_mode: String,
    /// Natural-language question (ignored by scorer but present in corpus)
    #[allow(dead_code)]
    #[serde(default)]
    pub question: Option<String>,
    pub canonical: CanonicalBlock,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CanonicalBlock {
    pub measures: Vec<String>,
    pub dimensions: Vec<String>,
    /// Measure/dim names that are known-wrong (lookalike traps etc.)
    #[serde(default)]
    pub rejected: Vec<String>,
}

// ---------------------------------------------------------------------------
// Candidate types
// ---------------------------------------------------------------------------

/// A file of per-task candidate calls — map of task_id -> ordered list.
#[derive(Debug, Clone, Deserialize)]
pub struct CandidateFile(pub BTreeMap<String, Vec<CandidateCall>>);

/// A single recorded candidate call.
#[derive(Debug, Clone, Deserialize)]
pub struct CandidateCall {
    /// Pre-resolved measures (free-form arm: from SQL parse; structured: from MQO fields)
    #[serde(default)]
    pub resolved_measures: Vec<String>,
    /// Pre-resolved dimensions
    #[serde(default)]
    pub resolved_dimensions: Vec<String>,
    /// MQO payload (structured arm only; absent for free-form candidates)
    #[serde(default)]
    pub mqo: Option<mqo_param_validator::BoundMqoInput>,
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

fn normalize(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '[' || *c == ']')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Path-correctness scorer (mirrors score_path_correctness.py).
///
/// Returns true iff:
/// - resolved_measures ⊇ canonical.measures
/// - resolved_dimensions ⊇ canonical.dimensions
/// - no resolved field is in canonical.rejected
pub fn score_path_correctness(call: &CandidateCall, canonical: &CanonicalBlock) -> bool {
    let rejected_norms: Vec<String> = canonical.rejected.iter().map(|s| normalize(s)).collect();

    for m in &call.resolved_measures {
        if rejected_norms.contains(&normalize(m)) {
            return false;
        }
    }
    for d in &call.resolved_dimensions {
        if rejected_norms.contains(&normalize(d)) {
            return false;
        }
    }

    let resolved_m: Vec<String> = call.resolved_measures.iter().map(|s| normalize(s)).collect();
    let resolved_d: Vec<String> = call.resolved_dimensions.iter().map(|s| normalize(s)).collect();

    canonical.measures.iter().all(|cm| resolved_m.contains(&normalize(cm)))
        && canonical
            .dimensions
            .iter()
            .all(|cd| resolved_d.contains(&normalize(cd)))
}

// ---------------------------------------------------------------------------
// Per-task result
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct TaskResult {
    pub freeform_pass_at_k: bool,
    pub freeform_pass_at_1: bool,
    pub freeform_first_try_valid: bool,

    pub structured_pass_at_k: bool,
    pub structured_pass_at_1: bool,
    pub structured_first_try_valid: bool,

    pub caught_by_validator: usize,
}

// ---------------------------------------------------------------------------
// Report types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ModeStats {
    pub failure_mode: String,
    pub task_count: usize,
    pub freeform_pass_at_1: f64,
    pub freeform_pass_at_k: f64,
    pub freeform_first_try_valid_rate: f64,
    pub structured_pass_at_1: f64,
    pub structured_pass_at_k: f64,
    pub structured_first_try_valid_rate: f64,
    pub caught_by_validator: usize,
    pub verdict: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BenchReport {
    pub k: usize,
    pub per_mode: Vec<ModeStats>,
    pub overall: OverallStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct OverallStats {
    pub task_count: usize,
    pub freeform_pass_at_1: f64,
    pub freeform_pass_at_k: f64,
    pub freeform_first_try_valid_rate: f64,
    pub structured_pass_at_1: f64,
    pub structured_pass_at_k: f64,
    pub structured_first_try_valid_rate: f64,
    pub total_caught_by_validator: usize,
}

// ---------------------------------------------------------------------------
// Core engine
// ---------------------------------------------------------------------------

pub fn run_bench(
    corpus: &[CorpusTask],
    freeform: &CandidateFile,
    structured: &CandidateFile,
    catalog: &CatalogSnapshot,
    k: usize,
) -> BenchReport {
    let mut mode_map: BTreeMap<String, Vec<TaskResult>> = BTreeMap::new();

    for task in corpus {
        let result = score_task(task, freeform, structured, catalog, k);
        mode_map
            .entry(task.failure_mode.clone())
            .or_default()
            .push(result);
    }

    let mut per_mode: Vec<ModeStats> = Vec::new();

    for (mode, results) in &mode_map {
        let n = results.len();
        if n == 0 {
            continue;
        }

        let ff_p1 = results.iter().filter(|r| r.freeform_pass_at_1).count();
        let ff_pk = results.iter().filter(|r| r.freeform_pass_at_k).count();
        let ff_ftv = results.iter().filter(|r| r.freeform_first_try_valid).count();
        let st_p1 = results.iter().filter(|r| r.structured_pass_at_1).count();
        let st_pk = results.iter().filter(|r| r.structured_pass_at_k).count();
        let st_ftv = results.iter().filter(|r| r.structured_first_try_valid).count();
        let caught: usize = results.iter().map(|r| r.caught_by_validator).sum();

        per_mode.push(ModeStats {
            failure_mode: mode.clone(),
            task_count: n,
            freeform_pass_at_1: ff_p1 as f64 / n as f64,
            freeform_pass_at_k: ff_pk as f64 / n as f64,
            freeform_first_try_valid_rate: ff_ftv as f64 / n as f64,
            structured_pass_at_1: st_p1 as f64 / n as f64,
            structured_pass_at_k: st_pk as f64 / n as f64,
            structured_first_try_valid_rate: st_ftv as f64 / n as f64,
            caught_by_validator: caught,
            verdict: format!(
                "{mode}: structured caught {caught}/{n} the free-form arm executed"
            ),
        });
    }

    let total = corpus.len();
    let all_results: Vec<&TaskResult> = mode_map.values().flatten().collect();

    let overall = if total == 0 {
        OverallStats {
            task_count: 0,
            freeform_pass_at_1: 0.0,
            freeform_pass_at_k: 0.0,
            freeform_first_try_valid_rate: 0.0,
            structured_pass_at_1: 0.0,
            structured_pass_at_k: 0.0,
            structured_first_try_valid_rate: 0.0,
            total_caught_by_validator: 0,
        }
    } else {
        OverallStats {
            task_count: total,
            freeform_pass_at_1: all_results.iter().filter(|r| r.freeform_pass_at_1).count() as f64
                / total as f64,
            freeform_pass_at_k: all_results.iter().filter(|r| r.freeform_pass_at_k).count() as f64
                / total as f64,
            freeform_first_try_valid_rate: all_results
                .iter()
                .filter(|r| r.freeform_first_try_valid)
                .count() as f64
                / total as f64,
            structured_pass_at_1: all_results
                .iter()
                .filter(|r| r.structured_pass_at_1)
                .count() as f64
                / total as f64,
            structured_pass_at_k: all_results
                .iter()
                .filter(|r| r.structured_pass_at_k)
                .count() as f64
                / total as f64,
            structured_first_try_valid_rate: all_results
                .iter()
                .filter(|r| r.structured_first_try_valid)
                .count() as f64
                / total as f64,
            total_caught_by_validator: all_results.iter().map(|r| r.caught_by_validator).sum(),
        }
    };

    BenchReport { k, per_mode, overall }
}

pub fn score_task(
    task: &CorpusTask,
    freeform: &CandidateFile,
    structured: &CandidateFile,
    catalog: &CatalogSnapshot,
    k: usize,
) -> TaskResult {
    let mut result = TaskResult::default();

    // --- Free-form arm ---
    if let Some(ff_calls) = freeform.0.get(&task.id) {
        let first_k: Vec<&CandidateCall> = ff_calls.iter().take(k).collect();
        if let Some(first) = first_k.first() {
            result.freeform_first_try_valid =
                !first.resolved_measures.is_empty() || !first.resolved_dimensions.is_empty();
            result.freeform_pass_at_1 = score_path_correctness(first, &task.canonical);
        }
        result.freeform_pass_at_k = first_k
            .iter()
            .any(|c| score_path_correctness(c, &task.canonical));
    }

    // --- Structured arm ---
    if let Some(st_calls) = structured.0.get(&task.id) {
        let first_k: Vec<&CandidateCall> = st_calls.iter().take(k).collect();
        let mut first_seen = false;
        let mut pass_at_k = false;
        let mut caught_count = 0usize;

        for call in &first_k {
            let validator_rejected = if let Some(mqo) = &call.mqo {
                !mqo_param_validator::validate(mqo, catalog).is_empty()
            } else {
                false
            };

            if !first_seen {
                first_seen = true;
                result.structured_first_try_valid = !validator_rejected;
                result.structured_pass_at_1 = !validator_rejected
                    && score_path_correctness(call, &task.canonical);
            }

            if validator_rejected {
                caught_count += 1;
            } else if !pass_at_k && score_path_correctness(call, &task.canonical) {
                pass_at_k = true;
            }
        }

        result.structured_pass_at_k = pass_at_k;
        result.caught_by_validator = caught_count;
    }

    result
}

// ---------------------------------------------------------------------------
// Markdown renderer (also used by main)
// ---------------------------------------------------------------------------

pub fn render_markdown(report: &BenchReport) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "# MQO Param-Q Bench Report (pass@{})\n\n",
        report.k
    ));
    out.push_str("## Per Failure Mode\n\n");
    out.push_str("| Mode | Tasks | FF pass@1 | FF pass@k | St pass@1 | St pass@k | Caught | FF ftv | St ftv |\n");
    out.push_str("|------|-------|-----------|-----------|-----------|-----------|--------|--------|--------|\n");

    for m in &report.per_mode {
        out.push_str(&format!(
            "| {} | {} | {:.3} | {:.3} | {:.3} | {:.3} | {} | {:.3} | {:.3} |\n",
            m.failure_mode,
            m.task_count,
            m.freeform_pass_at_1,
            m.freeform_pass_at_k,
            m.structured_pass_at_1,
            m.structured_pass_at_k,
            m.caught_by_validator,
            m.freeform_first_try_valid_rate,
            m.structured_first_try_valid_rate,
        ));
    }

    out.push_str("\n## Verdicts\n\n");
    for m in &report.per_mode {
        out.push_str(&format!("- {}\n", m.verdict));
    }

    out.push_str("\n## Overall\n\n");
    let o = &report.overall;
    out.push_str(&format!("- Tasks: {}\n", o.task_count));
    out.push_str(&format!(
        "- Free-form  pass@1: {:.3} | pass@k: {:.3} | first-try-valid: {:.3}\n",
        o.freeform_pass_at_1, o.freeform_pass_at_k, o.freeform_first_try_valid_rate
    ));
    out.push_str(&format!(
        "- Structured pass@1: {:.3} | pass@k: {:.3} | first-try-valid: {:.3}\n",
        o.structured_pass_at_1, o.structured_pass_at_k, o.structured_first_try_valid_rate
    ));
    out.push_str(&format!(
        "- Total caught by validator: {}\n",
        o.total_caught_by_validator
    ));

    out
}
