//! AC3: Scoring semantics match `score_path_correctness.py` — zero drift on shared fixture.
//!
//! The fixture encodes cases exercised by the Python scorer; expected results
//! are derived from the same logic and verified here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::doc_markdown)]

use mqoguard_regression_harness::corpus::normalise_task;
use mqoguard_regression_harness::score::score_normalised;
use mqoguard_regression_harness::types::{CanonicalBlock, Task, TrajectoryRecord};

/// Helper to build a minimal `TrajectoryRecord`.
fn rec(task_id: &str, answer: &str, error: Option<&str>, rows_n: Option<i64>, sql: &str) -> TrajectoryRecord {
    let rows = rows_n.map(|n| {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "n".to_owned(),
            serde_json::Value::Number(serde_json::Number::from(n)),
        );
        vec![m]
    });
    TrajectoryRecord {
        task_id: task_id.to_owned(),
        mcp: Some("nonprod".to_owned()),
        rollout: Some(0),
        model: None,
        model_id: None,
        final_sql: Some(sql.to_owned()),
        rows,
        row_count: rows_n,
        error: error.map(str::to_owned),
        answer: Some(answer.to_owned()),
        n_retries: Some(0),
    }
}

/// Helper to build a `path_incompatible` task.
fn pi_task(id: &str) -> Task {
    Task {
        id: id.to_owned(),
        failure_mode: Some("path_incompatible".to_owned()),
        canonical: Some(CanonicalBlock {
            approach: Some("reject the query".to_owned()),
            measures: vec![],
            dimensions: vec![],
        }),
        rejected: vec![],
        required_calcs: None,
        required_dims: None,
        forbidden_constructs: None,
        also_acceptable_calcs: vec![],
        expected_min_rows: None,
        expected_numeric: None,
    }
}

/// Helper to build a normal task with required calc + dim.
fn normal_task(id: &str, mode: &str, measure: &str, dim: &str, rejected: &str) -> Task {
    Task {
        id: id.to_owned(),
        failure_mode: Some(mode.to_owned()),
        canonical: Some(CanonicalBlock {
            approach: Some("answer normally".to_owned()),
            measures: vec![measure.to_owned()],
            dimensions: vec![dim.to_owned()],
        }),
        rejected: vec![rejected.to_owned()],
        required_calcs: None,
        required_dims: None,
        forbidden_constructs: None,
        also_acceptable_calcs: vec![],
        expected_min_rows: None,
        expected_numeric: None,
    }
}

// ---- path_incompatible cases ----

/// Python scorer: answer with "path_incompatible" keyword → PASS.
#[test]
fn pi_keyword_in_answer_passes() {
    let task = pi_task("t1");
    let norm = normalise_task(&task);
    let r = rec("t1", "this query is path_incompatible", None, None, "");
    let s = score_normalised(&r, &norm);
    assert!(s.pass_by_path, "rejection keyword in answer should pass: {}", s.why_path);
}

/// Python scorer: answer with "cannot answer" → PASS.
#[test]
fn pi_cannot_answer_keyword_passes() {
    let task = pi_task("t2");
    let norm = normalise_task(&task);
    let r = rec(
        "t2",
        "I cannot answer this query as it is not compatible",
        None,
        None,
        "",
    );
    let s = score_normalised(&r, &norm);
    assert!(s.pass_by_path, "cannot answer keyword should pass: {}", s.why_path);
}

/// Python scorer: rows returned on path_incompatible → FAIL (fabrication).
#[test]
fn pi_fabricated_rows_fails() {
    let task = pi_task("t3");
    let norm = normalise_task(&task);
    let r = rec("t3", "here are results", None, Some(10), "SELECT 1");
    let s = score_normalised(&r, &norm);
    assert!(!s.pass_by_path, "rows on PI task should fail: {}", s.why_path);
    assert!(
        s.why_path.contains("fabricated"),
        "why should mention fabricated: {}",
        s.why_path
    );
}

/// Python scorer: error + no rows on path_incompatible → PASS (engine rejection).
#[test]
fn pi_error_no_rows_passes() {
    let task = pi_task("t4");
    let norm = normalise_task(&task);
    let r = rec("t4", "the query failed", Some("path not found in catalog"), None, "");
    let s = score_normalised(&r, &norm);
    assert!(s.pass_by_path, "error+no rows should pass (engine rejection): {}", s.why_path);
}

/// Python scorer: no keyword, no error, no rows → FAIL.
#[test]
fn pi_no_signal_fails() {
    let task = pi_task("t5");
    let norm = normalise_task(&task);
    let r = rec("t5", "I tried but got nothing", None, None, "");
    let s = score_normalised(&r, &norm);
    assert!(!s.pass_by_path, "no rejection signal should fail: {}", s.why_path);
}

// ---- dim/calc path check cases ----

/// Python scorer: SQL contains required measure AND required dim → PASS.
#[test]
fn normal_correct_measure_and_dim_passes() {
    let task = normal_task("n1", "wrong_date_role", "Store Sales", "Sold Date Year", "Ship Date Year");
    let norm = normalise_task(&task);
    let r = rec(
        "n1",
        "ok",
        None,
        Some(1),
        r#"SELECT "Store Sales", "Sold Date Year" FROM t"#,
    );
    let s = score_normalised(&r, &norm);
    assert!(s.pass_by_path, "correct measure+dim should pass: {}", s.why_path);
}

/// Python scorer: SQL contains forbidden construct → FAIL.
#[test]
fn normal_forbidden_construct_fails() {
    let task = normal_task("n2", "wrong_date_role", "Store Sales", "Sold Date Year", "Ship Date Year");
    let norm = normalise_task(&task);
    let r = rec(
        "n2",
        "ok",
        None,
        Some(1),
        r#"SELECT "Store Sales", "Ship Date Year" FROM t"#,
    );
    let s = score_normalised(&r, &norm);
    assert!(!s.pass_by_path, "forbidden construct should fail: {}", s.why_path);
}

/// Python scorer: SQL has required measure but missing dim → FAIL (dim check, enforce_dims=True).
#[test]
fn normal_missing_dim_fails() {
    let task = normal_task("n3", "wrong_date_role", "Store Sales", "Sold Date Year", "Ship Date Year");
    let norm = normalise_task(&task);
    // SQL has the measure but not the required dim.
    let r = rec(
        "n3",
        "ok",
        None,
        Some(1),
        r#"SELECT "Store Sales", "Calendar Year" FROM t"#,
    );
    let s = score_normalised(&r, &norm);
    assert!(!s.pass_by_path, "missing required_dim should fail: {}", s.why_path);
    assert!(
        s.why_path.contains("required_dim"),
        "why should mention required_dim: {}",
        s.why_path
    );
}

/// Python scorer: SQL has required dim but missing measure → FAIL.
#[test]
fn normal_missing_calc_fails() {
    let task = normal_task("n4", "wrong_date_role", "Store Sales", "Sold Date Year", "Ship Date Year");
    let norm = normalise_task(&task);
    let r = rec(
        "n4",
        "ok",
        None,
        Some(1),
        r#"SELECT "Web Sales", "Sold Date Year" FROM t"#,
    );
    let s = score_normalised(&r, &norm);
    assert!(!s.pass_by_path, "missing required_calc should fail: {}", s.why_path);
}

/// Python scorer: error in record → FAIL for non-PI tasks (rule 1).
#[test]
fn normal_error_fails() {
    let task = normal_task("n5", "lookalike_measure", "Store Sales", "Sold Date Year", "Ship Date Year");
    let norm = normalise_task(&task);
    let r = rec("n5", "error occurred", Some("scan failed"), None, "");
    let s = score_normalised(&r, &norm);
    assert!(!s.pass_by_path, "error should fail non-PI task: {}", s.why_path);
}
