use mqo_bench_history::ingest::ingest;
use std::fs;
use tempfile::TempDir;

fn make_bench_json(accuracy: f64) -> String {
    serde_json::json!({
        "aggregate": {
            "accuracy_delta_pp": accuracy,
            "entity_error_delta_pp": -5.0,
            "latency_delta_ms": -100.0,
            "token_delta": -50.0
        },
        "per_question": [{"q": "1"}]
    })
    .to_string()
}

#[test]
fn ac2_zero_prior_records_no_baseline() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("runs.jsonl");

    fs::write(&bench_file, make_bench_json(80.0)).unwrap();

    let result = ingest(&bench_file, &history_file, 5, 5.0).unwrap();
    assert!(!result.has_baseline, "0 prior records: no baseline expected");
    assert!(result.metrics.is_empty());
}

#[test]
fn ac2_one_prior_record_no_baseline() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("runs.jsonl");

    fs::write(&bench_file, make_bench_json(80.0)).unwrap();

    // First ingest — 0 prior, no baseline
    let r1 = ingest(&bench_file, &history_file, 5, 5.0).unwrap();
    assert!(!r1.has_baseline);

    // Second ingest — 1 prior, still no baseline (need >=2)
    let r2 = ingest(&bench_file, &history_file, 5, 5.0).unwrap();
    assert!(!r2.has_baseline, "1 prior record: still no baseline");
}

#[test]
fn ac2_two_prior_records_has_baseline() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("runs.jsonl");

    fs::write(&bench_file, make_bench_json(80.0)).unwrap();

    ingest(&bench_file, &history_file, 5, 5.0).unwrap();
    ingest(&bench_file, &history_file, 5, 5.0).unwrap();
    let r3 = ingest(&bench_file, &history_file, 5, 5.0).unwrap();
    assert!(r3.has_baseline, "2 prior records: baseline should be available");
    assert!(!r3.metrics.is_empty());
}
