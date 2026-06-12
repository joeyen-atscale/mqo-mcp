//! Scoring logic — mirrors `score_record` and `_path_incompat_score` from
//! `runner/score_path_correctness.py`.
//!
//! This module is deterministic and has no I/O.

use crate::corpus::{normalise_task, NormalisedTask, REJECTION_KEYWORDS};
use crate::types::{RecordScore, Task, TrajectoryRecord};
use std::collections::HashMap;

// ---- SQL identifier matching ----

/// Extract lower-cased quoted SQL identifiers from a SQL string.
/// Mirrors Python `sql_identifiers`.
fn sql_identifiers(sql: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            i += 1;
            let mut ident = String::new();
            while i < bytes.len() {
                if bytes[i] == b'"' {
                    // Check for escaped quote ""
                    if i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                        ident.push('"');
                        i += 2;
                    } else {
                        i += 1;
                        break;
                    }
                } else {
                    ident.push(bytes[i] as char);
                    i += 1;
                }
            }
            ids.push(ident.to_lowercase());
        } else {
            i += 1;
        }
    }
    ids
}

/// Return true if the ref looks like a simple identifier (no operators).
fn is_identifier_like(r: &str) -> bool {
    !r.is_empty() && !r.contains(['(', ')', ',', '*', '/', '+', '-'])
}

/// Case-insensitive substring search. Returns the first matching needle.
fn contains_any_ci<'a>(haystack: &str, needles: &[&'a str]) -> Option<&'a str> {
    if haystack.is_empty() || needles.is_empty() {
        return None;
    }
    let h = haystack.to_lowercase();
    needles
        .iter()
        .find(|&&n| h.contains(&n.to_lowercase() as &str))
        .copied()
}

/// Match scorer refs against SQL, preferring quoted-identifier matching.
/// Mirrors Python `contains_sql_ref`.
#[must_use]
pub fn contains_sql_ref(sql: &str, refs: &[String]) -> Option<String> {
    if sql.is_empty() || refs.is_empty() {
        return None;
    }
    let identifiers = sql_identifiers(sql);
    let mut fallback: Vec<&str> = Vec::new();

    for r in refs {
        let cleaned = r.trim();
        if cleaned.is_empty() {
            continue;
        }
        if is_identifier_like(cleaned) {
            if identifiers.contains(&cleaned.to_lowercase()) {
                return Some(r.clone());
            }
        } else {
            fallback.push(cleaned);
        }
    }

    if !fallback.is_empty() {
        if let Some(m) = contains_any_ci(sql, &fallback) {
            return Some(m.to_owned());
        }
    }
    None
}

// ---- path_incompatible pass rule ----

/// Pass/fail for a `path_incompatible` task.
/// Mirrors Python `_path_incompat_score` in `value_and_path` mode.
///
/// Pass iff:
///   - Rejection keyword in (answer + error text), OR
///   - error is non-empty AND rows is empty
fn path_incompat_score(rec: &TrajectoryRecord) -> (bool, String) {
    let answer = rec.answer.as_deref().unwrap_or("");
    let error = rec.error.as_deref().unwrap_or("");
    let rows = rec.rows.as_deref().unwrap_or(&[]);

    let combined_lower = format!("{answer} {error}").to_lowercase();

    for &kw in REJECTION_KEYWORDS {
        if combined_lower.contains(kw) {
            return (true, format!("correct rejection (matched \"{kw}\")"));
        }
    }

    // Engine-rejection backdoor: error non-empty AND no rows.
    if !error.is_empty() && rows.is_empty() {
        let snippet: String = error.chars().take(80).collect();
        return (
            true,
            format!("engine rejected impossible join: {snippet}"),
        );
    }

    if !rows.is_empty() {
        return (
            false,
            format!("fabricated rows on path_incompatible task ({} rows)", rows.len()),
        );
    }

    (false, "no rejection signal".to_owned())
}

// ---- first numeric ----

fn first_numeric(rows: &[HashMap<String, serde_json::Value>]) -> Option<f64> {
    let row = rows.first()?;
    for v in row.values() {
        if let Some(n) = v.as_f64() {
            return Some(n);
        }
    }
    None
}

fn approx_equal(a: f64, b: f64, tol_pct: f64) -> bool {
    let threshold = (b.abs() * tol_pct).max(0.01);
    (a - b).abs() <= threshold
}

// ---- main scoring entry point ----

/// Score one trajectory record against its normalised task.
/// Mirrors Python `score_record` in `value_and_path` mode with `enforce_dims=true`.
#[must_use]
pub fn score_record(rec: &TrajectoryRecord, task: &Task) -> RecordScore {
    let norm = normalise_task(task);
    score_normalised(rec, &norm)
}

/// Score against a pre-normalised task (used by acceptance tests directly).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn score_normalised(rec: &TrajectoryRecord, norm: &NormalisedTask) -> RecordScore {
    let task_id = rec.task_id.clone();
    let mcp = rec.mcp.clone();
    let rollout = rec.rollout;

    // path_incompatible tasks have a different pass rule.
    if norm.is_path_incompatible {
        let (passed, why) = path_incompat_score(rec);
        return RecordScore {
            task_id,
            mcp,
            rollout,
            pass_by_path: passed,
            why_path: why,
        };
    }

    let sql = rec.final_sql.as_deref().unwrap_or("");
    let error = rec.error.as_deref().unwrap_or("");
    let rows = rec.rows.as_deref().unwrap_or(&[]);
    let row_count = rows.len();

    let expected_min_rows = norm.expected_min_rows;
    let expected_numeric = norm.expected_numeric;
    let tol_pct = 0.01_f64; // default 1%

    // --- pass_by_number (drives pass_by_path for non-rejection tasks) ---
    let (by_number, by_number_why) = if !error.is_empty() {
        let snippet: String = error.chars().take(60).collect();
        (false, format!("error: {snippet}"))
    } else if expected_min_rows == 0 {
        // Task expects no rows.
        let has_nonzero = rows.iter().any(|row| {
            row.values().any(|v| {
                v.as_f64()
                    .is_some_and(|n| n != 0.0 && !n.is_nan() && !n.is_infinite())
            })
        });
        if has_nonzero {
            (false, "expected no rows but got non-zero data".to_owned())
        } else {
            (true, String::new())
        }
    } else if i64::try_from(row_count).is_ok_and(|rc| rc < expected_min_rows) {
        (
            false,
            format!("got {row_count} rows, expected >= {expected_min_rows}"),
        )
    } else if let Some(expected) = expected_numeric {
        match first_numeric(rows) {
            Some(n) if approx_equal(n, expected, tol_pct) => (true, String::new()),
            Some(n) => (false, format!("got {n}, expected ≈ {expected}")),
            None => (false, "no numeric in result rows".to_owned()),
        }
    } else {
        // No numeric GT: check that first row isn't all-null / non-numeric.
        let has_numeric = rows
            .iter()
            .any(|row| row.values().any(|v| v.as_f64().is_some()));
        if !rows.is_empty() && !has_numeric {
            (false, "no numeric in any row".to_owned())
        } else {
            (true, String::new())
        }
    };

    // pass_by_path starts at pass_by_number, then applies path gates.
    let mut by_path = by_number;
    let mut by_path_why = by_number_why;

    // Forbidden constructs check.
    if by_path && !norm.forbidden_constructs.is_empty() {
        if let Some(m) = contains_sql_ref(sql, &norm.forbidden_constructs) {
            by_path = false;
            by_path_why = format!("forbidden_construct: {m:?}");
        }
    }

    // Required calcs check.
    if by_path && !norm.required_calcs.is_empty() {
        let mut all_required = norm.required_calcs.clone();
        all_required.extend_from_slice(&norm.also_acceptable_calcs);
        if contains_sql_ref(sql, &all_required).is_none() {
            by_path = false;
            let preview: Vec<&String> = norm.required_calcs.iter().take(3).collect();
            by_path_why = format!("missing required_calc; expected one of: {preview:?}");
        }
    }

    // Required dims check (mirrors enforce_dims=True, added 2026-05-06).
    if by_path && !norm.required_dims.is_empty() && contains_sql_ref(sql, &norm.required_dims).is_none() {
        by_path = false;
        let preview: Vec<&String> = norm.required_dims.iter().take(3).collect();
        by_path_why = format!("missing required_dim; expected one of: {preview:?}");
    }

    RecordScore {
        task_id,
        mcp,
        rollout,
        pass_by_path: by_path,
        why_path: if by_path {
            "ok".to_owned()
        } else {
            by_path_why
        },
    }
}

/// Bucket a `why_path` string into a short failure-reason key.
/// Mirrors Python failure-reason bucketing in the reporter.
#[must_use]
pub fn bucket_reason(why: &str) -> &str {
    if why.starts_with("forbidden_construct") {
        "forbidden_construct"
    } else if why.starts_with("missing required_calc") {
        "missing_required_calc"
    } else if why.starts_with("missing required_dim") {
        "missing_required_dim"
    } else if why.starts_with("error:") {
        "engine_error"
    } else if why.starts_with("got") && why.contains("expected") {
        "wrong_numeric"
    } else if why == "no rejection signal" || why.starts_with("fabricated rows") {
        why
    } else if why.len() > 40 {
        &why[..40]
    } else {
        why
    }
}
