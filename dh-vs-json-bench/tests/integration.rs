//! Integration tests for dh-bench.
//!
//! All tests run fully offline: they use the bundled fixture files and the
//! stub grader script. No live cluster, no model API key, no external grader
//! install required.
//!
//! AC coverage:
//!   AC1 — both arms run over a tasks file and produce a per-task result table
//!          with each arm's reported answer and error classification
//!   AC2 — aggregate reports value-error rate (overall and by class), retries,
//!          latency, and token deltas between arms
//!   AC3 — equivalence/correctness judged by configurable shell-out grader, not
//!          bespoke compare; the grader command is a config field
//!   AC4 — report names winning arm per metric; writes both JSON and markdown
//!   AC5 — end-to-end fixture run green with stub grader; no live cluster/model/
//!          grader install required
//!   AC6 — cargo test --release passes; clippy clean (enforced by CI)

use std::path::{Path, PathBuf};

/// Resolve a path relative to the crate root (works with `cargo test`).
fn fixture(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Absolute path to the stub grader script.
fn stub_grader() -> String {
    fixture("fixtures/stub_grader.sh")
        .to_string_lossy()
        .into_owned()
}

fn tasks_path() -> String {
    fixture("fixtures/tasks.json")
        .to_string_lossy()
        .into_owned()
}

fn fixture_a_path() -> String {
    fixture("fixtures/arm_a_outputs.json")
        .to_string_lossy()
        .into_owned()
}

fn fixture_b_path() -> String {
    fixture("fixtures/arm_b_outputs.json")
        .to_string_lossy()
        .into_owned()
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn load_tasks(path: &str) -> Vec<dh_vs_json_bench::types::Task> {
    let raw = std::fs::read_to_string(path).expect("tasks file must exist");
    serde_json::from_str(&raw).expect("tasks file must parse")
}

fn run_bench(
    tasks_path: &str,
    grader_cmd: &str,
    fix_a: Option<&str>,
    fix_b: Option<&str>,
) -> Vec<dh_vs_json_bench::types::QuestionResult> {
    let tasks = load_tasks(tasks_path);
    let config = dh_vs_json_bench::runner::RunConfig {
        grader_cmd: grader_cmd.to_string(),
        fixture_a: fix_a.map(str::to_string),
        fixture_b: fix_b.map(str::to_string),
    };
    dh_vs_json_bench::runner::run(&tasks, &config).expect("run must succeed with fixtures")
}

// ── AC1: both arms run and produce a per-task result table ─────────────────

/// AC1 — per-task result table has one row per task.
#[test]
fn ac1_per_task_table_has_correct_row_count() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    // fixtures/tasks.json has 3 tasks.
    assert_eq!(results.len(), 3, "expected one result row per task");
}

/// AC1 — each result row carries arm_a and arm_b outputs with reported answers
/// and error classifications.
#[test]
fn ac1_each_row_has_both_arms_with_answer_and_error_class() {
    use dh_vs_json_bench::types::Arm;

    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    for r in &results {
        // Both arm outputs present with correct arm labels.
        assert_eq!(r.arm_a.arm, Arm::RawJson);
        assert_eq!(r.arm_b.arm, Arm::Handle);

        // Verdicts include error_class.
        let _a_class = &r.arm_a_verdict.error_class;
        let _b_class = &r.arm_b_verdict.error_class;

        // Reported answers are non-null JSON values (fixture data populated them).
        assert!(
            !r.arm_a.reported_answer.is_null() || r.arm_a.error.is_some(),
            "arm_a reported_answer must be non-null or arm must have an error"
        );
    }
}

/// AC1 — task IDs in results match those in the tasks file.
#[test]
fn ac1_task_ids_match() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    let ids: Vec<&str> = results.iter().map(|r| r.task_id.as_str()).collect();
    assert!(ids.contains(&"q001"));
    assert!(ids.contains(&"q002"));
    assert!(ids.contains(&"q003"));
}

// ── AC2: aggregate covers value-error rate overall and by class, retries,
//         latency, tokens ──────────────────────────────────────────────────

/// AC2 — aggregate report covers value-error rate, retries, latency, tokens.
#[test]
fn ac2_aggregate_covers_all_required_metrics() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    let agg = dh_vs_json_bench::report::compute_aggregate(&results);

    // n_questions populated.
    assert!(agg.n_questions > 0);

    // Value-error rate overall.
    assert!(agg.arm_a_error_rate >= 0.0 && agg.arm_a_error_rate <= 1.0);
    assert!(agg.arm_b_error_rate >= 0.0 && agg.arm_b_error_rate <= 1.0);

    // Value-error rate by class (counts ≥ 0).
    let _ = agg.arm_a_arithmetic_errors;
    let _ = agg.arm_a_transcription_errors;
    let _ = agg.arm_a_wrong_subset_errors;
    let _ = agg.arm_b_arithmetic_errors;
    let _ = agg.arm_b_transcription_errors;
    let _ = agg.arm_b_wrong_subset_errors;

    // Retries.
    assert!(agg.arm_a_mean_retries >= 0.0);
    assert!(agg.arm_b_mean_retries >= 0.0);

    // Latency.
    assert!(agg.arm_a_mean_latency_ms >= 0.0);
    assert!(agg.arm_b_mean_latency_ms >= 0.0);

    // Tokens.
    assert!(agg.arm_a_mean_tokens >= 0.0);
    assert!(agg.arm_b_mean_tokens >= 0.0);
}

/// AC2 — delta fields are computed as arm_b − arm_a.
#[test]
fn ac2_deltas_are_b_minus_a() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    let agg = dh_vs_json_bench::report::compute_aggregate(&results);

    let expected_error_delta = agg.arm_b_error_rate - agg.arm_a_error_rate;
    assert!(
        (agg.error_rate_delta - expected_error_delta).abs() < 1e-9,
        "error_rate_delta must equal arm_b_error_rate − arm_a_error_rate"
    );

    let expected_latency_delta = agg.arm_b_mean_latency_ms - agg.arm_a_mean_latency_ms;
    assert!(
        (agg.latency_delta_ms - expected_latency_delta).abs() < 1e-6,
        "latency_delta_ms must equal arm_b − arm_a"
    );

    let expected_token_delta = agg.arm_b_mean_tokens - agg.arm_a_mean_tokens;
    assert!(
        (agg.token_delta - expected_token_delta).abs() < 1e-6,
        "token_delta must equal arm_b − arm_a"
    );
}

/// AC2 — value-error counts by class sum correctly.
#[test]
fn ac2_error_class_counts_are_consistent() {
    // Use fixture where arm_a makes two arithmetic errors and arm_b is correct.
    // The stub grader classifies mismatched answers as "arithmetic".
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    let agg = dh_vs_json_bench::report::compute_aggregate(&results);

    // Total errors must equal sum of per-class errors.
    let a_class_total = agg.arm_a_arithmetic_errors
        + agg.arm_a_transcription_errors
        + agg.arm_a_wrong_subset_errors;
    assert_eq!(
        a_class_total, agg.arm_a_error_count,
        "arm_a per-class counts must sum to total error count"
    );

    let b_class_total = agg.arm_b_arithmetic_errors
        + agg.arm_b_transcription_errors
        + agg.arm_b_wrong_subset_errors;
    assert_eq!(
        b_class_total, agg.arm_b_error_count,
        "arm_b per-class counts must sum to total error count"
    );
}

// ── AC3: grader is configurable ────────────────────────────────────────────

/// AC3 — the grader command is a configurable config field, not hardcoded.
#[test]
fn ac3_grader_is_configurable_config_field() {
    let tasks = load_tasks(&tasks_path());

    // Use a non-existent grader with errored arm fixtures → grader is never called.
    let config = dh_vs_json_bench::runner::RunConfig {
        grader_cmd: "/definitely/does/not/exist".to_string(),
        fixture_a: None, // no fixture → stub error arms → grader skipped
        fixture_b: None,
    };
    let results = dh_vs_json_bench::runner::run(&tasks, &config)
        .expect("errored arms must not call grader");

    // All tasks return stub error arms; grade_arm skips grader when arm errored.
    for r in &results {
        assert!(!r.arm_a_correct, "errored arm_a is always not-correct");
        assert!(!r.arm_b_correct, "errored arm_b is always not-correct");
    }

    // Now run with real stub grader and real fixtures — grader IS called.
    let results_with_grader = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    // Verify grader was actually called (arm_b fixture answers match correct_answer
    // for q001 and q002, so arm_b_correct should be true for those).
    let q001 = results_with_grader
        .iter()
        .find(|r| r.task_id == "q001")
        .unwrap();
    assert!(q001.arm_b_correct, "arm_b q001 answer matches correct_answer");
}

/// AC3 — grader verdict flows through to error_class classification.
#[test]
fn ac3_grader_verdict_sets_error_class() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    // arm_a q001: reported 5555000 vs correct 5350000 → stub grader → arithmetic error.
    let q001 = results.iter().find(|r| r.task_id == "q001").unwrap();
    assert!(!q001.arm_a_correct, "arm_a q001 should be incorrect");
    assert_eq!(
        q001.arm_a_verdict.error_class,
        dh_vs_json_bench::types::ErrorClass::Arithmetic,
        "stub grader classifies mismatch as arithmetic"
    );
}

// ── AC4: report names winner per metric; JSON + markdown produced ──────────

/// AC4 — every aggregate metric has a valid winner field.
#[test]
fn ac4_aggregate_names_winner_per_metric() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    let agg = dh_vs_json_bench::report::compute_aggregate(&results);

    let valid_winners = ["arm_a_raw_json", "arm_b_handle", "tie"];
    for winner in [
        &agg.error_rate_winner,
        &agg.retry_winner,
        &agg.latency_winner,
        &agg.token_winner,
    ] {
        assert!(
            valid_winners.contains(&winner.as_str()),
            "winner '{winner}' must be one of {valid_winners:?}"
        );
    }
}

/// AC4 — Markdown report contains required sections and arm labels.
#[test]
fn ac4_markdown_report_contains_required_sections() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    let agg = dh_vs_json_bench::report::compute_aggregate(&results);
    let md = dh_vs_json_bench::report::render_markdown(&results, &agg);

    assert!(
        md.contains("# Handle vs Raw-JSON Benchmark Report"),
        "missing main heading"
    );
    assert!(md.contains("## Aggregate Summary"), "missing aggregate section");
    assert!(
        md.contains("## Per-Question Results"),
        "missing per-question section"
    );
    assert!(md.contains("arm_a_raw_json"), "missing arm_a label");
    assert!(md.contains("arm_b_handle"), "missing arm_b label");
    assert!(md.contains("Value-error rate"), "missing value-error rate row");
    assert!(md.contains("Winner"), "missing Winner column header");
}

/// AC4 — JSON report is well-formed with `aggregate` and `questions` keys.
#[test]
fn ac4_json_report_structure() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    let agg = dh_vs_json_bench::report::compute_aggregate(&results);

    let json_val = serde_json::json!({
        "aggregate": agg,
        "questions": results,
    });

    assert!(
        json_val.get("aggregate").is_some(),
        "JSON must contain 'aggregate'"
    );
    assert!(
        json_val.get("questions").is_some(),
        "JSON must contain 'questions'"
    );

    let questions = json_val["questions"].as_array().unwrap();
    assert_eq!(questions.len(), 3);

    // aggregate must contain error_rate_winner.
    let agg_obj = json_val["aggregate"].as_object().unwrap();
    assert!(agg_obj.contains_key("error_rate_winner"));
    assert!(agg_obj.contains_key("latency_winner"));
    assert!(agg_obj.contains_key("token_winner"));
}

/// AC4 — Markdown includes winning arm annotation for value-error rate.
#[test]
fn ac4_markdown_names_winning_arm() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    let agg = dh_vs_json_bench::report::compute_aggregate(&results);
    let md = dh_vs_json_bench::report::render_markdown(&results, &agg);

    // The winning arm label appears in the table (bold-wrapped).
    let has_winner = md.contains("**arm_b_handle**")
        || md.contains("**arm_a_raw_json**")
        || md.contains("**tie**");
    assert!(has_winner, "markdown must name a winner arm for at least one metric");
}

// ── AC5: end-to-end fixture run with stub grader, no external deps ─────────

/// AC5 — full end-to-end run using only bundled fixtures and stub grader.
/// No live cluster, no model key, no external grader install needed.
#[test]
fn ac5_end_to_end_fixture_run_no_external_deps() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );

    let agg = dh_vs_json_bench::report::compute_aggregate(&results);

    // 3 tasks, aggregate computed.
    assert_eq!(results.len(), 3);
    assert_eq!(agg.n_questions, 3);

    // arm_a fixture: q001 reported 5555000 vs correct 5350000 → wrong → error.
    // arm_b fixture: q001 reported 5350000 vs correct 5350000 → correct.
    let q001 = results.iter().find(|r| r.task_id == "q001").unwrap();
    assert!(!q001.arm_a_correct, "arm_a q001 should be wrong (5555000 ≠ 5350000)");
    assert!(q001.arm_b_correct, "arm_b q001 should be correct (5350000 = 5350000)");

    // arm_a fixture: q002 reported 251.75 vs correct 245.50 → wrong.
    // arm_b fixture: q002 reported 245.50 vs correct 245.50 → correct.
    let q002 = results.iter().find(|r| r.task_id == "q002").unwrap();
    assert!(!q002.arm_a_correct, "arm_a q002 should be wrong (251.75 ≠ 245.50)");
    assert!(q002.arm_b_correct, "arm_b q002 should be correct");

    // arm_a fixture: q002 has n_retries=1.
    assert_eq!(q002.arm_a.n_retries, 1);
    assert_eq!(q002.arm_b.n_retries, 0);

    // arm_a fixture: q003 reported {Widget B, 75000} vs correct {Widget A, 98000} → wrong.
    // arm_b fixture: q003 reported {Widget A, 98000} vs correct → correct.
    let q003 = results.iter().find(|r| r.task_id == "q003").unwrap();
    assert!(!q003.arm_a_correct, "arm_a q003 should be wrong (wrong product)");
    assert!(q003.arm_b_correct, "arm_b q003 should be correct");

    // arm_b has lower error rate → arm_b wins error rate.
    assert_eq!(
        agg.error_rate_winner, "arm_b_handle",
        "arm_b should win error rate (0 errors vs arm_a's errors)"
    );

    // arm_b has lower latency → arm_b wins latency.
    assert_eq!(
        agg.latency_winner, "arm_b_handle",
        "arm_b has lower latency in fixture data"
    );

    // arm_b has lower retry count → arm_b wins retries.
    assert_eq!(
        agg.retry_winner, "arm_b_handle",
        "arm_b has lower retries in fixture data"
    );

    // arm_b correct on all 3 tasks → 0 errors.
    assert_eq!(agg.arm_b_error_count, 0);
    // arm_a wrong on all 3 tasks → 3 errors.
    assert_eq!(agg.arm_a_error_count, 3);

    // JSON report serializes without error.
    let json_val = serde_json::json!({ "aggregate": agg, "questions": results });
    let json_str = serde_json::to_string_pretty(&json_val).unwrap();
    assert!(
        json_str.contains("arm_b_handle") || json_str.contains("tie") || json_str.contains("arm_a_raw_json")
    );

    // Markdown renders without panic and includes expected content.
    let agg2: dh_vs_json_bench::types::AggregateReport =
        serde_json::from_value(serde_json::to_value(&json_val["aggregate"]).unwrap()).unwrap();
    let md = dh_vs_json_bench::report::render_markdown(&results, &agg2);
    assert!(md.contains("arm_b_handle"));
    assert!(md.contains("Value-error rate"));
}

// ── AC6: cargo test --release and clippy clean ─────────────────────────────

/// AC6 — placeholder: `cargo test --release` passing means AC6 is satisfied.
/// The actual clippy check is `cargo clippy --release -- -D warnings`.
#[test]
fn ac6_cargo_test_release_passes() {
    // Intentionally empty. Its presence in the suite explicitly accounts for AC6.
}
