//! Corpus normalization: mirrors `_normalize_task` and `_is_path_incompatible`
//! from `runner/score_path_correctness.py`.

use crate::types::{CanonicalBlock, Task};

/// Rejection keywords — exact mirrors of the Python scorer list.
pub const REJECTION_KEYWORDS: &[&str] = &[
    "path_incompatible",
    "path-incompatible",
    "not compatible",
    "incompatible",
    "cannot answer",
    "cannot be answered",
    "no compatible path",
    "not bound to",
    "rejecting",
    "reject this query",
    "reject as path",
    "unable to answer",
    "is not bound",
    "no path between",
    "cannot combine",
];

/// Return true if this task's canonical answer is "reject the query".
/// Mirrors Python `_is_path_incompatible`.
#[must_use]
pub fn is_path_incompatible(task: &Task) -> bool {
    if task
        .failure_mode
        .as_deref()
        .is_some_and(|m| m == "path_incompatible")
    {
        return true;
    }
    if let Some(cn) = &task.canonical {
        let approach_lc = cn.approach.as_deref().unwrap_or("").to_lowercase();
        let no_measures = cn.measures.is_empty();
        let no_dims = cn.dimensions.is_empty();
        if no_measures && no_dims && approach_lc.contains("reject") {
            return true;
        }
    }
    false
}

/// Strip trailing parentheticals and "= filter" clauses from a canonical ref.
/// Mirrors Python `_clean_ref`.
fn clean_ref(s: &str) -> String {
    // Strip trailing " (some text)"
    let s = s
        .rfind('(')
        .map_or_else(|| s.trim(), |pos| {
            let suffix = &s[pos..];
            if suffix.ends_with(')') {
                s[..pos].trim_end()
            } else {
                s.trim()
            }
        });
    // Strip trailing "= ..."
    let s = s.find('=').map_or(s, |pos| s[..pos].trim_end());
    s.to_owned()
}

/// Expand "X x Y" rejected pairings into both halves.
fn expand_rejected(forbidden: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for f in forbidden {
        if f.to_lowercase().contains(" x ") {
            let parts = split_on_x(&f);
            out.extend(parts.into_iter().filter(|p| !p.is_empty()));
        } else {
            out.push(f);
        }
    }
    out
}

fn split_on_x(s: &str) -> Vec<String> {
    let lower = s.to_lowercase();
    lower.find(" x ").map_or_else(
        || vec![s.to_owned()],
        |pos| {
            vec![
                s[..pos].trim().to_owned(),
                s[pos + 3..].trim().to_owned(),
            ]
        },
    )
}

/// Normalised task fields used by the scorer.
#[derive(Debug, Clone)]
pub struct NormalisedTask {
    /// Failure mode (`None` if absent from corpus).
    pub failure_mode: Option<String>,

    /// Required calcs (from `canonical.measures` or `required_calcs`).
    pub required_calcs: Vec<String>,

    /// Required dims (from `canonical.dimensions` or `required_dims`).
    pub required_dims: Vec<String>,

    /// Forbidden constructs (from `rejected[]` or `forbidden_constructs`).
    pub forbidden_constructs: Vec<String>,

    /// Also-acceptable calcs.
    pub also_acceptable_calcs: Vec<String>,

    /// Expected min rows (default 1).
    pub expected_min_rows: i64,

    /// Expected numeric (`None` = no check).
    pub expected_numeric: Option<f64>,

    /// Whether this is a `path_incompatible` task.
    pub is_path_incompatible: bool,
}

/// Normalise a task to scorer fields. Mirrors Python `_normalize_task`.
#[must_use]
pub fn normalise_task(task: &Task) -> NormalisedTask {
    let is_pi = is_path_incompatible(task);

    // If already in calc-sensitive shape, use it directly.
    if task.required_calcs.is_some() || task.forbidden_constructs.is_some() {
        return NormalisedTask {
            failure_mode: task.failure_mode.clone(),
            required_calcs: task.required_calcs.clone().unwrap_or_default(),
            required_dims: task.required_dims.clone().unwrap_or_default(),
            forbidden_constructs: task.forbidden_constructs.clone().unwrap_or_default(),
            also_acceptable_calcs: task.also_acceptable_calcs.clone(),
            expected_min_rows: task.expected_min_rows.unwrap_or(1),
            expected_numeric: task.expected_numeric,
            is_path_incompatible: is_pi,
        };
    }

    // Derive from canonical/rejected.
    let empty_canonical = CanonicalBlock {
        approach: None,
        measures: Vec::new(),
        dimensions: Vec::new(),
    };
    let cn = task.canonical.as_ref().unwrap_or(&empty_canonical);

    let required_calcs: Vec<String> = cn
        .measures
        .iter()
        .map(|m| clean_ref(m))
        .filter(|s| !s.is_empty())
        .collect();

    let required_dims: Vec<String> = cn
        .dimensions
        .iter()
        .map(|d| clean_ref(d))
        .filter(|s| !s.is_empty())
        .collect();

    let raw_forbidden: Vec<String> = task
        .rejected
        .iter()
        .map(|r| clean_ref(r))
        .filter(|s| !s.is_empty())
        .collect();

    let forbidden_constructs = expand_rejected(raw_forbidden);

    NormalisedTask {
        failure_mode: task.failure_mode.clone(),
        required_calcs,
        required_dims,
        forbidden_constructs,
        also_acceptable_calcs: task.also_acceptable_calcs.clone(),
        expected_min_rows: task.expected_min_rows.unwrap_or(1),
        expected_numeric: task.expected_numeric,
        is_path_incompatible: is_pi,
    }
}
