// Acceptance tests AC1-AC8 for mqo-paramq-bench
//
// All tests are fully offline and deterministic (no network, no LLM).
// Fixture files live in tests/fixtures/.

use mqo_param_validator::{BoundMqoInput, CatalogSnapshot, MqoDimensionRef, MqoMeasureRef};
use mqo_paramq_bench::{
    run_bench, score_path_correctness, score_task, CandidateCall, CandidateFile, CanonicalBlock,
    CorpusTask,
};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_catalog() -> CatalogSnapshot {
    let raw =
        std::fs::read_to_string("tests/fixtures/catalog.json").expect("catalog fixture missing");
    serde_json::from_str(&raw).expect("catalog parse")
}

fn load_corpus() -> Vec<CorpusTask> {
    let raw =
        std::fs::read_to_string("tests/fixtures/corpus.json").expect("corpus fixture missing");
    serde_json::from_str(&raw).expect("corpus parse")
}

fn load_freeform() -> CandidateFile {
    let raw = std::fs::read_to_string("tests/fixtures/freeform_candidates.json")
        .expect("freeform fixture missing");
    CandidateFile(serde_json::from_str(&raw).expect("freeform parse"))
}

fn load_structured() -> CandidateFile {
    let raw = std::fs::read_to_string("tests/fixtures/structured_candidates.json")
        .expect("structured fixture missing");
    CandidateFile(serde_json::from_str(&raw).expect("structured parse"))
}

fn simple_call(measures: Vec<&str>, dimensions: Vec<&str>) -> CandidateCall {
    CandidateCall {
        resolved_measures: measures.into_iter().map(String::from).collect(),
        resolved_dimensions: dimensions.into_iter().map(String::from).collect(),
        mqo: None,
    }
}

fn simple_task(id: &str, mode: &str, measures: Vec<&str>, dimensions: Vec<&str>, rejected: Vec<&str>) -> CorpusTask {
    CorpusTask {
        id: id.to_string(),
        failure_mode: mode.to_string(),
        question: None,
        canonical: CanonicalBlock {
            measures: measures.into_iter().map(String::from).collect(),
            dimensions: dimensions.into_iter().map(String::from).collect(),
            rejected: rejected.into_iter().map(String::from).collect(),
        },
    }
}

// ---------------------------------------------------------------------------
// AC1: Given corpus + both candidate files + catalog, emits per-failure-mode
//      pass@1 and pass@k as fractions in [0,1] for both arms, plus overall.
// ---------------------------------------------------------------------------
#[test]
fn acceptance_ac1_pass_at_k_fractions_in_range() {
    let corpus = load_corpus();
    let freeform = load_freeform();
    let structured = load_structured();
    let catalog = load_catalog();

    let report = run_bench(&corpus, &freeform, &structured, &catalog, 2);

    // Must have per_mode entries (non-empty corpus)
    assert!(!report.per_mode.is_empty(), "per_mode should not be empty");

    // All fractions must be in [0, 1]
    for m in &report.per_mode {
        assert!(
            (0.0..=1.0).contains(&m.freeform_pass_at_1),
            "freeform_pass_at_1 out of range for mode {}",
            m.failure_mode
        );
        assert!(
            (0.0..=1.0).contains(&m.freeform_pass_at_k),
            "freeform_pass_at_k out of range for mode {}",
            m.failure_mode
        );
        assert!(
            (0.0..=1.0).contains(&m.structured_pass_at_1),
            "structured_pass_at_1 out of range for mode {}",
            m.failure_mode
        );
        assert!(
            (0.0..=1.0).contains(&m.structured_pass_at_k),
            "structured_pass_at_k out of range for mode {}",
            m.failure_mode
        );
        assert!(
            (0.0..=1.0).contains(&m.freeform_first_try_valid_rate),
            "freeform_first_try_valid_rate out of range for mode {}",
            m.failure_mode
        );
        assert!(
            (0.0..=1.0).contains(&m.structured_first_try_valid_rate),
            "structured_first_try_valid_rate out of range for mode {}",
            m.failure_mode
        );
    }

    // Overall fractions also in [0, 1]
    let o = &report.overall;
    assert!((0.0..=1.0).contains(&o.freeform_pass_at_1));
    assert!((0.0..=1.0).contains(&o.freeform_pass_at_k));
    assert!((0.0..=1.0).contains(&o.structured_pass_at_1));
    assert!((0.0..=1.0).contains(&o.structured_pass_at_k));

    // task_count must match corpus
    assert_eq!(o.task_count, corpus.len());
}

// ---------------------------------------------------------------------------
// AC2: A structured candidate whose MQO names a non-existent measure is
//      counted in caught_by_validator and NOT scored as a path pass.
// ---------------------------------------------------------------------------
#[test]
fn acceptance_ac2_nonexistent_measure_caught_not_scored() {
    let catalog = load_catalog();

    // Build a minimal corpus task with a correct answer
    let task = simple_task("ac2_task", "lookalike_measure", vec!["[Total Sales]"], vec!["[Date]"], vec![]);

    // Free-form: one correct candidate
    let mut ff_map = BTreeMap::new();
    ff_map.insert(
        "ac2_task".to_string(),
        vec![simple_call(vec!["[Total Sales]"], vec!["[Date]"])],
    );
    let freeform = CandidateFile(ff_map);

    // Structured: first candidate names a non-existent measure
    let bad_mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "[Totally Fake Measure XYZ]".to_string(),
        }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "[Date]".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        filters: vec![],
    };

    let bad_call = CandidateCall {
        resolved_measures: vec!["[Total Sales]".to_string()], // would be a path pass
        resolved_dimensions: vec!["[Date]".to_string()],
        mqo: Some(bad_mqo),
    };

    let mut st_map = BTreeMap::new();
    st_map.insert("ac2_task".to_string(), vec![bad_call]);
    let structured = CandidateFile(st_map);

    let result = score_task(&task, &freeform, &structured, &catalog, 1);

    // Must be caught
    assert_eq!(
        result.caught_by_validator, 1,
        "non-existent measure must be caught by validator"
    );
    // Must NOT be a path pass
    assert!(
        !result.structured_pass_at_1,
        "caught candidate must not be scored as pass@1"
    );
    assert!(
        !result.structured_pass_at_k,
        "caught candidate must not be scored as pass@k"
    );
}

// ---------------------------------------------------------------------------
// AC3: lookalike_measure task: free-form executes the lookalike (path fail),
//      structured is validator-caught => positive caught_by_validator delta.
// ---------------------------------------------------------------------------
#[test]
fn acceptance_ac3_lookalike_caught_delta() {
    let catalog = load_catalog();

    // task_001 in fixture: free-form arm's first candidate uses lookalike
    let corpus = load_corpus();
    let freeform = load_freeform();
    let structured = load_structured();

    let report = run_bench(&corpus, &freeform, &structured, &catalog, 2);

    let lookalike_mode = report
        .per_mode
        .iter()
        .find(|m| m.failure_mode == "lookalike_measure")
        .expect("lookalike_measure mode must be present");

    // The structured arm should have caught at least 1 across the two lookalike tasks
    assert!(
        lookalike_mode.caught_by_validator > 0,
        "structured arm must catch at least one lookalike; got {}",
        lookalike_mode.caught_by_validator
    );

    // The verdict string must reference the mode
    assert!(
        lookalike_mode.verdict.contains("lookalike_measure"),
        "verdict must reference the failure mode"
    );
}

// ---------------------------------------------------------------------------
// AC4: Structured-arm path scoring uses the identical canonical-block
//      contract as free-form — a task passing in one scorer passes in the other
//      with the same resolved path.
// ---------------------------------------------------------------------------
#[test]
fn acceptance_ac4_identical_path_scoring_contract() {
    // Build a canonical block and test that score_path_correctness is symmetric
    let canonical = CanonicalBlock {
        measures: vec!["[Total Sales]".to_string()],
        dimensions: vec!["[Date]".to_string()],
        rejected: vec!["[Total Sales Lookalike]".to_string()],
    };

    // Passing call
    let passing = simple_call(vec!["[Total Sales]"], vec!["[Date]"]);
    assert!(
        score_path_correctness(&passing, &canonical),
        "identical resolved path should pass in both arms"
    );

    // Failing call (uses rejected measure)
    let failing = simple_call(vec!["[Total Sales Lookalike]"], vec!["[Date]"]);
    assert!(
        !score_path_correctness(&failing, &canonical),
        "rejected measure must fail in both arms"
    );

    // Missing dimension => fail
    let missing_dim = simple_call(vec!["[Total Sales]"], vec![]);
    assert!(
        !score_path_correctness(&missing_dim, &canonical),
        "missing canonical dimension must fail"
    );

    // Missing measure => fail
    let missing_meas = simple_call(vec![], vec!["[Date]"]);
    assert!(
        !score_path_correctness(&missing_meas, &canonical),
        "missing canonical measure must fail"
    );
}

// ---------------------------------------------------------------------------
// AC5: first-try-valid-call rate in [0,1]; task whose first structured
//      candidate fails validator but second passes: no double-count of caught.
// ---------------------------------------------------------------------------
#[test]
fn acceptance_ac5_first_try_valid_no_double_count() {
    let catalog = load_catalog();

    let task = simple_task(
        "ac5_task",
        "lookalike_measure",
        vec!["[Total Sales]"],
        vec!["[Date]"],
        vec![],
    );

    let mut ff_map = BTreeMap::new();
    ff_map.insert(
        "ac5_task".to_string(),
        vec![simple_call(vec!["[Total Sales]"], vec!["[Date]"])],
    );
    let freeform = CandidateFile(ff_map);

    // First structured candidate: validator rejects (non-existent measure)
    // Second structured candidate: validator accepts + path correct
    let bad_mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "[Nonexistent ABC]".to_string(),
        }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "[Date]".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        filters: vec![],
    };
    let good_mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "[Total Sales]".to_string(),
        }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "[Date]".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        filters: vec![],
    };

    let bad_call = CandidateCall {
        resolved_measures: vec!["[Total Sales]".to_string()],
        resolved_dimensions: vec!["[Date]".to_string()],
        mqo: Some(bad_mqo),
    };
    let good_call = CandidateCall {
        resolved_measures: vec!["[Total Sales]".to_string()],
        resolved_dimensions: vec!["[Date]".to_string()],
        mqo: Some(good_mqo),
    };

    let mut st_map = BTreeMap::new();
    st_map.insert("ac5_task".to_string(), vec![bad_call, good_call]);
    let structured = CandidateFile(st_map);

    let result = score_task(&task, &freeform, &structured, &catalog, 2);

    // First try failed validator => structured_first_try_valid = false
    assert!(
        !result.structured_first_try_valid,
        "first structured candidate fails validator => not first-try-valid"
    );

    // Only the FIRST candidate was caught (no double-count)
    assert_eq!(
        result.caught_by_validator, 1,
        "only one candidate caught, not double-counted"
    );

    // Second candidate passes => pass@k true
    assert!(
        result.structured_pass_at_k,
        "second valid candidate should contribute to pass@k"
    );

    // pass@1 false (first was caught)
    assert!(
        !result.structured_pass_at_1,
        "pass@1 should be false since first candidate was caught"
    );

    // first-try-valid rate in [0,1] when run through bench
    let corpus = vec![task];
    let report = run_bench(&corpus, &freeform, &structured, &catalog, 2);
    let mode = &report.per_mode[0];
    assert!(
        (0.0..=1.0).contains(&mode.structured_first_try_valid_rate),
        "first-try-valid-rate must be in [0,1]"
    );
    // Rate should be 0 (one task, first failed)
    assert_eq!(mode.structured_first_try_valid_rate, 0.0);
}

// ---------------------------------------------------------------------------
// AC6: Fully offline and deterministic — a second run on fixtures is identical.
// ---------------------------------------------------------------------------
#[test]
fn acceptance_ac6_deterministic_output() {
    let corpus = load_corpus();
    let freeform = load_freeform();
    let structured = load_structured();
    let catalog = load_catalog();

    let report1 = run_bench(&corpus, &freeform, &structured, &catalog, 3);
    let report2 = run_bench(&corpus, &freeform, &structured, &catalog, 3);

    let json1 = serde_json::to_string(&report1).expect("serialize 1");
    let json2 = serde_json::to_string(&report2).expect("serialize 2");

    assert_eq!(json1, json2, "two runs on the same inputs must be byte-identical");
}

// ---------------------------------------------------------------------------
// AC7: A failure mode with zero tasks is omitted from the report.
// ---------------------------------------------------------------------------
#[test]
fn acceptance_ac7_empty_mode_omitted() {
    // Build a corpus with only one failure mode
    let corpus = vec![simple_task(
        "ac7_task",
        "lookalike_measure",
        vec!["[Total Sales]"],
        vec!["[Date]"],
        vec![],
    )];

    let mut ff_map = BTreeMap::new();
    ff_map.insert(
        "ac7_task".to_string(),
        vec![simple_call(vec!["[Total Sales]"], vec!["[Date]"])],
    );
    let freeform = CandidateFile(ff_map);

    let mut st_map = BTreeMap::new();
    st_map.insert(
        "ac7_task".to_string(),
        vec![simple_call(vec!["[Total Sales]"], vec!["[Date]"])],
    );
    let structured = CandidateFile(st_map);

    let catalog = load_catalog();
    let report = run_bench(&corpus, &freeform, &structured, &catalog, 1);

    // Only one mode present
    assert_eq!(report.per_mode.len(), 1);
    assert_eq!(report.per_mode[0].failure_mode, "lookalike_measure");

    // Modes like "wrong_hierarchy_level" (not in corpus) must NOT appear
    let has_empty_mode = report
        .per_mode
        .iter()
        .any(|m| m.failure_mode == "wrong_hierarchy_level");
    assert!(
        !has_empty_mode,
        "mode with zero tasks must be omitted from report"
    );
}

// ---------------------------------------------------------------------------
// AC8: cargo test + clippy pass (this test simply exists; verified via build)
// ---------------------------------------------------------------------------
#[test]
fn acceptance_ac8_placeholder() {
    // AC8 is verified by running `cargo test --release` and
    // `cargo clippy --all-targets -- -D warnings` in CI.
    // This placeholder ensures the test file compiles and the module is exercised.
    let _ = "AC8: build and clippy verification pass";
}
