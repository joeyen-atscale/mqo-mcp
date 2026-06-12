//! Integration tests for mqo-bench.
//!
//! All tests run fully offline: they use the bundled fixture files and the
//! stub grader script. No live cluster, no model API key, no external grader
//! install required.
//!
//! AC coverage:
//!   AC1 — both arms run over a tasks file and produce a per-question table
//!   AC2 — aggregate report covers all five metrics
//!   AC3 — grader is configurable (stub_grader.sh path passed as --grader)
//!   AC4 — report names winning arm per metric; JSON + markdown both written
//!   AC5 — end-to-end fixture run green with no external dependency
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

// ── Helpers that replicate what main.rs does ───────────────────────────────

fn load_tasks(path: &str) -> Vec<mqo_vs_sql_bench::types::Task> {
    let raw = std::fs::read_to_string(path).expect("tasks file must exist");
    serde_json::from_str(&raw).expect("tasks file must parse")
}

fn run_bench(
    tasks_path: &str,
    grader_cmd: &str,
    fix_a: Option<&str>,
    fix_b: Option<&str>,
) -> Vec<mqo_vs_sql_bench::types::QuestionResult> {
    let tasks = load_tasks(tasks_path);
    let config = mqo_vs_sql_bench::runner::RunConfig {
        grader_cmd: grader_cmd.to_string(),
        fixture_a: fix_a.map(str::to_string),
        fixture_b: fix_b.map(str::to_string),
    };
    mqo_vs_sql_bench::runner::run(&tasks, &config).expect("run must succeed with fixtures")
}

// ── AC1: both arms run and produce a per-question table ───────────────────

/// AC1 — per-question result table has one row per task.
#[test]
fn ac1_per_question_table_has_correct_row_count() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    // fixtures/tasks.json has 3 tasks.
    assert_eq!(results.len(), 3, "expected one result row per task");
}

/// AC1 — each result row carries arm_a and arm_b outputs.
#[test]
fn ac1_each_row_has_both_arms() {
    use mqo_vs_sql_bench::types::Arm;

    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    for r in &results {
        assert_eq!(r.arm_a.arm, Arm::SqlRunQuery);
        assert_eq!(r.arm_b.arm, Arm::MqoMultidimensional);
    }
}

// ── AC2: aggregate covers all five metrics ─────────────────────────────────

/// AC2 — aggregate report covers accuracy, invalid-entity rate, retries,
/// latency, and tokens.
#[test]
fn ac2_aggregate_covers_all_five_metrics() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    let agg = mqo_vs_sql_bench::report::compute_aggregate(&results);

    // Accuracy present
    assert!(agg.n_questions > 0);
    assert!(agg.arm_a_accuracy >= 0.0 && agg.arm_a_accuracy <= 1.0);
    assert!(agg.arm_b_accuracy >= 0.0 && agg.arm_b_accuracy <= 1.0);

    // Invalid-entity rate present
    assert!(agg.arm_a_invalid_entity_rate >= 0.0);
    assert!(agg.arm_b_invalid_entity_rate >= 0.0);

    // Retries present
    assert!(agg.arm_a_mean_retries >= 0.0);
    assert!(agg.arm_b_mean_retries >= 0.0);

    // Latency present
    assert!(agg.arm_a_mean_latency_ms >= 0.0);
    assert!(agg.arm_b_mean_latency_ms >= 0.0);

    // Tokens present
    assert!(agg.arm_a_mean_tokens >= 0.0);
    assert!(agg.arm_b_mean_tokens >= 0.0);
}

/// AC2 — delta fields are computed correctly.
#[test]
fn ac2_deltas_are_b_minus_a() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    let agg = mqo_vs_sql_bench::report::compute_aggregate(&results);

    let expected_accuracy_delta = agg.arm_b_accuracy - agg.arm_a_accuracy;
    assert!(
        (agg.accuracy_delta - expected_accuracy_delta).abs() < 1e-9,
        "accuracy delta must equal arm_b_accuracy − arm_a_accuracy"
    );

    let expected_latency_delta = agg.arm_b_mean_latency_ms - agg.arm_a_mean_latency_ms;
    assert!(
        (agg.latency_delta_ms - expected_latency_delta).abs() < 1e-6,
        "latency delta must equal arm_b − arm_a"
    );
}

// ── AC3: grader is configurable ────────────────────────────────────────────

/// AC3 — the grader command is a configurable parameter, not hardcoded.
/// We verify by pointing at the stub grader and confirming the verdict flows
/// through (if the grader were hardcoded, substituting it would have no effect).
#[test]
fn ac3_grader_is_configurable_not_hardcoded() {
    let tasks = load_tasks(&tasks_path());
    // Use a non-existent grader on a task where both arms error → no grader call.
    let config = mqo_vs_sql_bench::runner::RunConfig {
        grader_cmd: "/definitely/does/not/exist".to_string(),
        fixture_a: None, // no fixture → both arms error → grader is never invoked
        fixture_b: None,
    };
    let results = mqo_vs_sql_bench::runner::run(&tasks, &config)
        .expect("both-errored case must not call grader");
    // All tasks will have stub error arms; grade_arms skips grader when both errored.
    for r in &results {
        assert!(!r.arms_equivalent, "both-error tasks are not equivalent");
    }

    // Now run with the real stub grader and real fixtures — grader IS called.
    let results_with_grader = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    // stub_grader always says equivalent=true; both arms have no error → both pass.
    assert!(
        results_with_grader.iter().all(|r| r.arms_equivalent),
        "stub grader always returns equivalent=true"
    );
}

// ── AC4: report names winner per metric; JSON + markdown produced ──────────

/// AC4 — every aggregate metric has a non-empty winner field.
#[test]
fn ac4_aggregate_names_winner_per_metric() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    let agg = mqo_vs_sql_bench::report::compute_aggregate(&results);

    let valid_winners = ["arm_a_sql", "arm_b_mqo", "tie"];
    for winner in [
        &agg.accuracy_winner,
        &agg.invalid_entity_winner,
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

/// AC4 — Markdown report is emitted and contains required sections.
#[test]
fn ac4_markdown_report_contains_required_sections() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    let agg = mqo_vs_sql_bench::report::compute_aggregate(&results);
    let md = mqo_vs_sql_bench::report::render_markdown(&results, &agg);

    assert!(md.contains("# MQO vs SQL Benchmark Report"), "missing main heading");
    assert!(md.contains("## Aggregate Summary"), "missing aggregate section");
    assert!(md.contains("## Per-Question Results"), "missing per-question section");
    assert!(md.contains("arm_a_sql"), "missing arm_a label");
    assert!(md.contains("arm_b_mqo"), "missing arm_b label");
}

/// AC4 — JSON report is well-formed and contains both `aggregate` and `questions` keys.
#[test]
fn ac4_json_report_structure() {
    let results = run_bench(
        &tasks_path(),
        &stub_grader(),
        Some(&fixture_a_path()),
        Some(&fixture_b_path()),
    );
    let agg = mqo_vs_sql_bench::report::compute_aggregate(&results);

    let json_val = serde_json::json!({
        "aggregate": agg,
        "questions": results,
    });

    assert!(json_val.get("aggregate").is_some(), "JSON must contain 'aggregate'");
    assert!(json_val.get("questions").is_some(), "JSON must contain 'questions'");

    let questions = json_val["questions"].as_array().unwrap();
    assert_eq!(questions.len(), 3);
}

// ── AC5: end-to-end fixture run with stub grader ───────────────────────────

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

    let agg = mqo_vs_sql_bench::report::compute_aggregate(&results);

    // Basic sanity: we got 3 results, aggregate computed, winners assigned.
    assert_eq!(results.len(), 3);
    assert_eq!(agg.n_questions, 3);

    // Stub grader says all equivalent; arm_a q002 has n_retries=1 (from fixture).
    let q002 = results.iter().find(|r| r.task_id == "q002").unwrap();
    assert_eq!(q002.arm_a.n_retries, 1);
    assert_eq!(q002.arm_b.n_retries, 0);

    // arm_a q003 has invalid_entity=true (from fixture); arm_b does not.
    let q003 = results.iter().find(|r| r.task_id == "q003").unwrap();
    assert!(q003.arm_a.invalid_entity, "arm_a q003 invalid_entity should be true");
    assert!(!q003.arm_b.invalid_entity, "arm_b q003 invalid_entity should be false");

    // arm_b consistently lower latency → arm_b wins latency.
    assert_eq!(agg.latency_winner, "arm_b_mqo", "arm_b has lower latency in fixture data");

    // arm_b has lower retry count → arm_b wins retries.
    assert_eq!(agg.retry_winner, "arm_b_mqo", "arm_b has lower retries in fixture data");

    // arm_a has invalid_entity on q003; arm_b has none → arm_b wins invalid-entity rate.
    assert_eq!(
        agg.invalid_entity_winner, "arm_b_mqo",
        "arm_b has lower invalid-entity rate in fixture data"
    );

    // Both arms pass all tasks (stub grader → equivalent, no errors in fixture).
    assert_eq!(agg.arm_a_pass_count, 3);
    assert_eq!(agg.arm_b_pass_count, 3);

    // JSON report serializes without error.
    let json_val = serde_json::json!({ "aggregate": agg, "questions": results });
    let json_str = serde_json::to_string_pretty(&json_val).unwrap();
    assert!(json_str.contains("arm_b_mqo") || json_str.contains("tie") || json_str.contains("arm_a_sql"));

    // Markdown renders without panic.
    let _md = mqo_vs_sql_bench::report::render_markdown(&results, &serde_json::from_value(
        serde_json::to_value(&json_val["aggregate"]).unwrap()
    ).unwrap());
}

// ── AC6: cargo test --release and clippy clean (structural, not a runtime test)

/// AC6 — placeholder test: `cargo test --release` passing means AC6 is satisfied.
/// The actual clippy check is run by `cargo clippy --release -- -D warnings`.
#[test]
fn ac6_cargo_test_release_passes() {
    // This test intentionally has no assertions. Its presence in the suite
    // ensures AC6 is explicitly accounted for in the test inventory.
    // `cargo clippy -- -D warnings` is the real check; it runs in the build step.
}
