//! Acceptance tests for the mqo-backend-live-harness.
//!
//! Library-level tests (AC1–AC7 below) run entirely offline using
//! FakeProbe + FakeEngine + FakeComparator.
//! CLI-level tests (corpus_* below) test the binary via assert_cmd.

use std::collections::HashMap;

use mqo_backend_live_harness::{
    comparator::FakeComparator,
    probe::FakeProbe,
    runner::{self, FakeEngine},
    Backend, BackendStatus, CheckOutcome, ParityOutcome, TestCase,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn two_cases() -> Vec<TestCase> {
    vec![
        TestCase {
            name: "scalar_total_store_sales".to_string(),
            mqo: serde_json::json!({"measures": ["store_sales.total_store_sales"]}),
            expected_value: Some(10_170_000_000.0),
        },
        TestCase {
            name: "store_sales_by_item_class".to_string(),
            mqo: serde_json::json!({"measures": ["store_sales.total_store_sales"], "dimensions": ["item.item_class"]}),
            expected_value: Some(10_170_000_000.0),
        },
    ]
}

fn exact_values_for(backends: &[Backend], cases: &[TestCase]) -> HashMap<(Backend, String), f64> {
    let mut m = HashMap::new();
    for &b in backends {
        for c in cases {
            m.insert((b, c.name.clone()), c.expected_value.unwrap());
        }
    }
    m
}

// ---------------------------------------------------------------------------
// AC1: SQL-only host — SQL passes, DAX skipped (rejected), MDX skipped (unreachable)
// ---------------------------------------------------------------------------

#[test]
fn ac1_sql_only_host_dax_mdx_skipped() {
    let cases = two_cases();
    let backends = vec![Backend::Sql, Backend::Dax, Backend::Mdx];

    let mut probe_map = HashMap::new();
    probe_map.insert(Backend::Sql, BackendStatus::Live);
    probe_map.insert(
        Backend::Dax,
        BackendStatus::Rejected {
            reason: "PGWire rejected EVALUATE (SQL-only host)".to_string(),
        },
    );
    probe_map.insert(
        Backend::Mdx,
        BackendStatus::Unreachable {
            reason: "XMLA :11111 unreachable".to_string(),
        },
    );
    let probe = FakeProbe::new(probe_map);
    let engine_values = exact_values_for(&[Backend::Sql], &cases);
    let engine = FakeEngine::new(engine_values);
    let comparator = FakeComparator::new(HashMap::new());

    let report = runner::run_harness(&backends, &cases, &probe, &engine, &comparator);

    let sql_results: Vec<_> = report
        .results
        .iter()
        .filter(|r| r.backend == Backend::Sql)
        .collect();
    assert!(
        sql_results.iter().all(|r| r.outcome == CheckOutcome::Pass),
        "SQL checks should all pass"
    );

    let dax_results: Vec<_> = report
        .results
        .iter()
        .filter(|r| r.backend == Backend::Dax)
        .collect();
    for r in &dax_results {
        match &r.outcome {
            CheckOutcome::Skip { reason } => {
                assert!(
                    reason.contains("rejected"),
                    "DAX skip reason should mention rejected, got: {reason}"
                );
            }
            other => panic!("expected DAX to be skipped, got {other:?}"),
        }
    }

    let mdx_results: Vec<_> = report
        .results
        .iter()
        .filter(|r| r.backend == Backend::Mdx)
        .collect();
    for r in &mdx_results {
        match &r.outcome {
            CheckOutcome::Skip { reason } => {
                assert!(
                    reason.contains("unreachable"),
                    "MDX skip reason should mention unreachable, got: {reason}"
                );
            }
            other => panic!("expected MDX to be skipped, got {other:?}"),
        }
    }

    assert!(report.is_success(), "report should be success (no failures)");
    assert!(report.skipped() > 0, "should have skips (got {})", report.skipped());
    assert_eq!(report.failed(), 0, "should have zero failures (got {})", report.failed());
}

// ---------------------------------------------------------------------------
// AC2: Report format — pass/skip/fail all appear, summary line correct
// ---------------------------------------------------------------------------

#[test]
fn ac2_report_format_all_outcomes() {
    let cases = vec![
        TestCase {
            name: "passing_case".to_string(),
            mqo: serde_json::json!({}),
            expected_value: Some(42.0),
        },
        TestCase {
            name: "failing_case".to_string(),
            mqo: serde_json::json!({}),
            expected_value: Some(100.0),
        },
    ];

    let mut probe_map = HashMap::new();
    probe_map.insert(Backend::Sql, BackendStatus::Live);
    probe_map.insert(
        Backend::Dax,
        BackendStatus::Unreachable {
            reason: "port closed".to_string(),
        },
    );
    let probe = FakeProbe::new(probe_map);

    let mut engine_values = HashMap::new();
    engine_values.insert((Backend::Sql, "passing_case".to_string()), 42.0);
    engine_values.insert((Backend::Sql, "failing_case".to_string()), 999.0);
    let engine = FakeEngine::new(engine_values);
    let comparator = FakeComparator::new(HashMap::new());

    let report = runner::run_harness(
        &[Backend::Sql, Backend::Dax],
        &cases,
        &probe,
        &engine,
        &comparator,
    );

    let rendered = report.render();
    assert!(
        rendered.contains('✅') && rendered.contains("passing_case"),
        "rendered should contain ✅ passing_case"
    );
    assert!(
        rendered.contains('❌') && rendered.contains("failing_case"),
        "rendered should contain ❌ failing_case"
    );
    assert!(rendered.contains('⏭'), "rendered should contain ⏭️");
    assert!(
        rendered.contains("passed") && rendered.contains("skipped") && rendered.contains("failed"),
        "summary line must mention passed/skipped/failed"
    );
    assert_eq!(report.passed(), 1);
    assert_eq!(report.failed(), 1);
    assert_eq!(report.skipped(), 2);
}

// ---------------------------------------------------------------------------
// AC3: DAX live — scalar + dim cases execute, assert, parity runs
// ---------------------------------------------------------------------------

#[test]
fn ac3_dax_live_executes_and_parity_runs() {
    let cases = two_cases();
    let backends = vec![Backend::Sql, Backend::Dax];
    let probe = FakeProbe::with_live(&[Backend::Sql, Backend::Dax]);
    let engine_values = exact_values_for(&[Backend::Sql, Backend::Dax], &cases);
    let engine = FakeEngine::new(engine_values.clone());
    let comparator = FakeComparator::new(engine_values);

    let report = runner::run_harness(&backends, &cases, &probe, &engine, &comparator);

    assert!(
        report.results.iter().all(|r| r.outcome == CheckOutcome::Pass),
        "all checks should pass when both backends are live and return correct values"
    );
    assert!(
        report.parity.contains(&ParityOutcome::Agreed),
        "parity should have at least one Agreed result"
    );
    assert!(report.is_success());
}

// ---------------------------------------------------------------------------
// AC4: Wrong value → ❌ fail + non-zero exit (tested via is_success == false)
// ---------------------------------------------------------------------------

#[test]
fn ac4_wrong_value_fails() {
    let cases = vec![TestCase {
        name: "scalar".to_string(),
        mqo: serde_json::json!({}),
        expected_value: Some(1000.0),
    }];

    let probe = FakeProbe::with_live(&[Backend::Sql]);
    let mut engine_values = HashMap::new();
    engine_values.insert((Backend::Sql, "scalar".to_string()), 999.0_f64);
    let engine = FakeEngine::new(engine_values);
    let comparator = FakeComparator::new(HashMap::new());

    let report = runner::run_harness(&[Backend::Sql], &cases, &probe, &engine, &comparator);

    let sql_result = report
        .results
        .iter()
        .find(|r| r.backend == Backend::Sql && r.case_name == "scalar")
        .unwrap();
    assert!(
        matches!(sql_result.outcome, CheckOutcome::Fail { .. }),
        "wrong value should produce Fail"
    );
    assert!(!report.is_success(), "is_success should be false on failure");
    assert_eq!(report.failed(), 1);
}

// ---------------------------------------------------------------------------
// AC5: Probe + comparator are traits — verified by custom inline impl
// ---------------------------------------------------------------------------

#[test]
fn ac5_custom_probe_and_comparator_inline() {
    use mqo_backend_live_harness::{comparator::ParityComparator, probe::CapabilityProbe};

    struct AlwaysLiveProbe;
    impl CapabilityProbe for AlwaysLiveProbe {
        fn probe(&self, _b: Backend) -> BackendStatus {
            BackendStatus::Live
        }
    }

    struct ConstComparator;
    impl ParityComparator for ConstComparator {
        fn compare(&self, _case: &TestCase, _backends: &[Backend]) -> ParityOutcome {
            ParityOutcome::Agreed
        }
    }

    let cases = vec![TestCase {
        name: "custom".to_string(),
        mqo: serde_json::json!({}),
        expected_value: Some(7.0),
    }];

    let probe = AlwaysLiveProbe;
    let mut vals = HashMap::new();
    vals.insert((Backend::Sql, "custom".to_string()), 7.0_f64);
    let engine = FakeEngine::new(vals);
    let comparator = ConstComparator;

    let report = runner::run_harness(&[Backend::Sql], &cases, &probe, &engine, &comparator);
    assert_eq!(report.passed(), 1);
    assert!(report.is_success());
}

// ---------------------------------------------------------------------------
// AC6: Adding a new case to JSON requires no code change (fixture round-trip)
// ---------------------------------------------------------------------------

#[test]
fn ac6_json_case_file_loads_without_code_change() {
    let fixture_path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/default_cases.json");
    let json = std::fs::read_to_string(fixture_path)
        .expect("default_cases.json should be readable");
    let cases: Vec<TestCase> =
        serde_json::from_str(&json).expect("default_cases.json should parse");
    assert!(cases.len() >= 2, "fixture should have ≥2 cases");
    for c in &cases {
        assert!(!c.name.is_empty(), "case name should not be empty");
    }
}

// ---------------------------------------------------------------------------
// AC7: Exit code rules
// ---------------------------------------------------------------------------

#[test]
fn ac7_exit_code_rules_all_skipped_is_success() {
    let cases = two_cases();
    let probe = FakeProbe::new(HashMap::new());
    let engine = FakeEngine::new(HashMap::new());
    let comparator = FakeComparator::new(HashMap::new());

    let report = runner::run_harness(
        &[Backend::Sql, Backend::Dax, Backend::Mdx],
        &cases,
        &probe,
        &engine,
        &comparator,
    );

    assert_eq!(report.failed(), 0);
    assert!(report.skipped() > 0);
    assert!(report.is_success());
}

#[test]
fn ac7_exit_code_rules_parity_divergence_fails() {
    let cases = vec![TestCase {
        name: "p".to_string(),
        mqo: serde_json::json!({}),
        expected_value: Some(100.0),
    }];

    let probe = FakeProbe::with_live(&[Backend::Sql, Backend::Dax]);
    let mut engine_values = HashMap::new();
    engine_values.insert((Backend::Sql, "p".to_string()), 100.0_f64);
    engine_values.insert((Backend::Dax, "p".to_string()), 100.0_f64);
    let engine = FakeEngine::new(engine_values);

    let mut comp_values = HashMap::new();
    comp_values.insert((Backend::Sql, "p".to_string()), 100.0_f64);
    comp_values.insert((Backend::Dax, "p".to_string()), 200.0_f64);
    let comparator = FakeComparator::new(comp_values);

    let report = runner::run_harness(
        &[Backend::Sql, Backend::Dax],
        &cases,
        &probe,
        &engine,
        &comparator,
    );

    assert_eq!(report.failed(), 0);
    assert!(
        report.parity.iter().any(|p| matches!(p, ParityOutcome::Diverged { .. })),
        "parity should diverge"
    );
    assert!(!report.is_success(), "divergence should cause is_success=false");
}

// ---------------------------------------------------------------------------
// Parity-only case: value lane skipped, parity runs (FR1 back-compat)
// ---------------------------------------------------------------------------

#[test]
fn parity_only_case_skips_value_assertion() {
    let cases = vec![TestCase {
        name: "parity_only".to_string(),
        mqo: serde_json::json!({}),
        expected_value: None,
    }];

    let probe = FakeProbe::with_live(&[Backend::Sql, Backend::Dax]);
    let engine = FakeEngine::new(HashMap::new()); // not called for None cases
    let mut comp_values = HashMap::new();
    comp_values.insert((Backend::Sql, "parity_only".to_string()), 42.0_f64);
    comp_values.insert((Backend::Dax, "parity_only".to_string()), 42.0_f64);
    let comparator = FakeComparator::new(comp_values);

    let report = runner::run_harness(
        &[Backend::Sql, Backend::Dax],
        &cases,
        &probe,
        &engine,
        &comparator,
    );

    assert_eq!(report.failed(), 0, "value assertion must not fire for parity-only case");
    assert!(
        report.parity.contains(&ParityOutcome::Agreed),
        "parity should run and agree"
    );
    assert!(report.is_success());
}

// ---------------------------------------------------------------------------
// Corpus input types: round-trip deserialization
// ---------------------------------------------------------------------------

#[test]
fn corpus_document_deserializes() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/test_corpus.json");
    let json = std::fs::read_to_string(fixture).expect("test_corpus.json readable");
    let doc: mqo_backend_live_harness::CorpusDocument =
        serde_json::from_str(&json).expect("test_corpus.json parses");
    assert_eq!(doc.version, "parity-corpus.v1");
    assert_eq!(doc.cases.len(), 2);
    assert_eq!(
        doc.cases[0].case_id,
        "tpcds_benchmark_model_total_store_sales__total"
    );
    assert!(doc.cases[0].mqo.is_object());
}

#[test]
fn corpus_case_maps_to_test_case() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/test_corpus.json");
    let json = std::fs::read_to_string(fixture).unwrap();
    let doc: mqo_backend_live_harness::CorpusDocument = serde_json::from_str(&json).unwrap();

    let cases: Vec<TestCase> = doc
        .cases
        .iter()
        .map(|c| TestCase {
            name: c.case_id.clone(),
            mqo: c.mqo.clone(),
            expected_value: None,
        })
        .collect();

    assert_eq!(cases.len(), 2);
    assert_eq!(
        cases[0].name,
        "tpcds_benchmark_model_total_store_sales__total"
    );
    assert!(cases[0].expected_value.is_none(), "corpus cases have no expected_value");
}

// ---------------------------------------------------------------------------
// CLI-level corpus tests (AC1, AC6–AC10) via assert_cmd
// ---------------------------------------------------------------------------

use assert_cmd::Command;

fn corpus_fixture(name: &str) -> String {
    format!("{}/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name)
}

/// AC1: Corpus loads, N records emitted, each carries case_id and build_id.
#[test]
fn cli_corpus_ac1_loads_and_emits_records() {
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("out.jsonl");
    let corpus = corpus_fixture("test_corpus.json");

    let mut cmd = Command::cargo_bin("mqo-live-harness").unwrap();
    cmd.env_remove("ATSCALE_PGWIRE_HOST")
        .env_remove("ATSCALE_XMLA_URL")
        .args([
            "--corpus",
            &corpus,
            "--build-id",
            "b-2026-06-10.1",
            "--out",
            out_path.to_str().unwrap(),
        ]);
    cmd.assert().success();

    let out = std::fs::read_to_string(&out_path).unwrap();
    let records: Vec<serde_json::Value> = out
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("valid JSON line"))
        .collect();

    assert_eq!(records.len(), 2, "expected 2 records for 2-case corpus");
    for r in &records {
        assert!(r["case_id"].is_string(), "record must have case_id");
        assert_eq!(r["build_id"].as_str().unwrap(), "b-2026-06-10.1");
    }
    assert_eq!(
        records[0]["case_id"].as_str().unwrap(),
        "tpcds_benchmark_model_total_store_sales__total"
    );
    assert_eq!(
        records[1]["case_id"].as_str().unwrap(),
        "tpcds_benchmark_model_total_store_sales__year"
    );
}

/// AC6: Empty corpus → 0 records, exit 0.
#[test]
fn cli_corpus_ac6_empty_corpus_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("out.jsonl");
    let corpus = corpus_fixture("test_corpus_empty.json");

    let mut cmd = Command::cargo_bin("mqo-live-harness").unwrap();
    cmd.env_remove("ATSCALE_PGWIRE_HOST")
        .env_remove("ATSCALE_XMLA_URL")
        .args([
            "--corpus",
            &corpus,
            "--build-id",
            "b-test",
            "--out",
            out_path.to_str().unwrap(),
        ]);
    cmd.assert().success();

    let out = std::fs::read_to_string(&out_path).unwrap();
    let non_empty: Vec<_> = out.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(non_empty.len(), 0, "empty corpus should emit 0 records");
}

/// AC7: No live backends → all AllSkipped records, exit 0.
#[test]
fn cli_corpus_ac7_no_live_backends_all_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("out.jsonl");
    let corpus = corpus_fixture("test_corpus.json");

    let mut cmd = Command::cargo_bin("mqo-live-harness").unwrap();
    cmd.env_remove("ATSCALE_PGWIRE_HOST")
        .env_remove("ATSCALE_XMLA_URL")
        .args([
            "--corpus",
            &corpus,
            "--build-id",
            "b-test",
            "--out",
            out_path.to_str().unwrap(),
        ]);
    cmd.assert().success();

    let out = std::fs::read_to_string(&out_path).unwrap();
    let records: Vec<serde_json::Value> = out
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();

    for r in &records {
        assert_eq!(
            r["overall"].as_str().unwrap(),
            "AllSkipped",
            "all records must be AllSkipped when no backends are reachable"
        );
    }
}

/// AC8: Malformed corpus → exit 2.
#[test]
fn cli_corpus_ac8_malformed_exits_2() {
    let corpus = corpus_fixture("test_corpus_malformed.json");
    let mut cmd = Command::cargo_bin("mqo-live-harness").unwrap();
    cmd.args(["--corpus", &corpus, "--build-id", "b-test"]);
    cmd.assert().failure().code(2);
}

/// AC8: Wrong version tag → exit 2.
#[test]
fn cli_corpus_ac8_wrong_version_exits_2() {
    let corpus = corpus_fixture("test_corpus_wrong_version.json");
    let mut cmd = Command::cargo_bin("mqo-live-harness").unwrap();
    cmd.args(["--corpus", &corpus, "--build-id", "b-test"]);
    cmd.assert().failure().code(2);
}

/// AC9: --cases and --corpus together → exit 2.
#[test]
fn cli_corpus_ac9_cases_and_corpus_mutually_exclusive() {
    let corpus = corpus_fixture("test_corpus.json");
    let cases = corpus_fixture("default_cases.json");
    let mut cmd = Command::cargo_bin("mqo-live-harness").unwrap();
    cmd.args([
        "--corpus", &corpus,
        "--cases", &cases,
        "--build-id", "b-test",
    ]);
    cmd.assert().failure().code(2);
}

/// AC10: --out without --build-id → exit 2.
#[test]
fn cli_corpus_ac10_out_without_build_id_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("out.jsonl");
    let corpus = corpus_fixture("test_corpus.json");

    let mut cmd = Command::cargo_bin("mqo-live-harness").unwrap();
    cmd.args([
        "--corpus",
        &corpus,
        "--out",
        out_path.to_str().unwrap(),
        // intentionally no --build-id
    ]);
    cmd.assert().failure().code(2);
}
