use mqo_bench_history::ingest::ingest;
use mqo_bench_history::types::HistoryRecord;
use std::fs;
use tempfile::TempDir;

fn make_bench_report_json(accuracy: f64) -> String {
    serde_json::json!({
        "aggregate": {
            "accuracy_delta_pp": accuracy,
            "entity_error_delta_pp": -5.0,
            "latency_delta_ms": -100.0,
            "token_delta": -50.0
        },
        "per_question": [
            {"question": "q1", "correct": true},
            {"question": "q2", "correct": false}
        ]
    })
    .to_string()
}

#[test]
fn ac1_ingest_creates_jsonl_with_one_line() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("runs.jsonl");

    fs::write(&bench_file, make_bench_report_json(80.0)).unwrap();

    let result = ingest(&bench_file, &history_file, 5, 5.0).unwrap();
    assert!(!result.skipped_duplicate);

    let content = fs::read_to_string(&history_file).unwrap();
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "Expected exactly 1 line in JSONL");

    let record: HistoryRecord = serde_json::from_str(lines[0]).expect("Valid HistoryRecord");
    assert!(!record.run_id.is_empty());
    assert!(!record.timestamp.is_empty());
    assert_eq!(record.per_question_count, 2);
    assert!(!record.task_file_hash.is_empty());
    assert_eq!(record.aggregate.accuracy_delta_pp, 80.0);
}

#[test]
fn ac1_creates_history_dir_if_absent() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("subdir").join("nested").join("runs.jsonl");

    fs::write(&bench_file, make_bench_report_json(80.0)).unwrap();

    // Dir does not exist yet
    assert!(!history_file.parent().unwrap().exists());

    let result = ingest(&bench_file, &history_file, 5, 5.0).unwrap();
    assert!(!result.skipped_duplicate);

    assert!(history_file.exists(), "History file should have been created");
}

#[test]
fn ac1_idempotent_on_same_run_id() {
    use mqo_bench_history::ingest::ingest_with_run_id;
    use mqo_bench_history::types::BenchReport;

    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("runs.jsonl");

    let json = make_bench_report_json(80.0);
    fs::write(&bench_file, &json).unwrap();

    let bench_bytes = fs::read(&bench_file).unwrap();
    let report: BenchReport = serde_json::from_slice(&bench_bytes).unwrap();
    let fixed_run_id = "fixed-run-id-1234".to_string();

    // First ingest with fixed run_id
    let report2: BenchReport = serde_json::from_slice(&bench_bytes).unwrap();
    let _r1 = ingest_with_run_id(fixed_run_id.clone(), &bench_bytes, report, &history_file, 5, 5.0).unwrap();

    // Second ingest with same run_id — should be skipped
    let r2 = ingest_with_run_id(fixed_run_id, &bench_bytes, report2, &history_file, 5, 5.0).unwrap();
    assert!(r2.skipped_duplicate, "Duplicate run_id should be skipped");

    let content = fs::read_to_string(&history_file).unwrap();
    let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "Duplicate run_id should not add a second line");
}
