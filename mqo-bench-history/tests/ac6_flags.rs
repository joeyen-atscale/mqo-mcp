use mqo_bench_history::ingest::ingest;
use mqo_bench_history::types::HistoryRecord;
use std::fs;
use std::io::Write;
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

fn write_n_records(history_file: &std::path::Path, n: usize, accuracy: f64) {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(history_file)
        .unwrap();

    for i in 0..n {
        let record = HistoryRecord {
            run_id: format!("prior-{}", i),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            aggregate: mqo_bench_history::types::AggMetrics {
                accuracy_delta_pp: accuracy,
                entity_error_delta_pp: -5.0,
                latency_delta_ms: -100.0,
                token_delta: -50.0,
            },
            per_question_count: 10,
            task_file_hash: "abc".to_string(),
        };
        writeln!(file, "{}", serde_json::to_string(&record).unwrap()).unwrap();
    }
}

#[test]
fn ac6_custom_history_file_path() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let custom_history = tmp.path().join("custom").join("history.jsonl");

    fs::write(&bench_file, make_bench_json(80.0)).unwrap();

    let result = ingest(&bench_file, &custom_history, 5, 5.0).unwrap();
    assert!(!result.skipped_duplicate);
    assert!(custom_history.exists(), "Custom history file should be created");
}

#[test]
fn ac6_custom_baseline_window() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("runs.jsonl");

    // 10 prior records, but window=3 → baseline over only last 3
    write_n_records(&history_file, 10, 80.0);
    fs::write(&bench_file, make_bench_json(73.0)).unwrap();

    let result = ingest(&bench_file, &history_file, 3, 5.0).unwrap();
    assert!(result.has_baseline);
    // With window=3 and all baseline=80, delta=-7 → REGRESS
    let acc = result.metrics.iter().find(|m| m.name == "accuracy_delta_pp").unwrap();
    assert_eq!(acc.verdict, mqo_bench_history::types::Verdict::Regress);
}

#[test]
fn ac6_custom_regress_threshold() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("runs.jsonl");

    // baseline=80, current=76 → delta=-4 pp
    write_n_records(&history_file, 5, 80.0);
    fs::write(&bench_file, make_bench_json(76.0)).unwrap();

    // With threshold=5.0: -4 pp → WARN (not regress)
    let result_warn = ingest(&bench_file, &history_file, 5, 5.0).unwrap();
    let acc_warn = result_warn.metrics.iter().find(|m| m.name == "accuracy_delta_pp").unwrap();
    assert_eq!(acc_warn.verdict, mqo_bench_history::types::Verdict::Warn);

    // With threshold=3.0: -4 pp → REGRESS
    let history_file2 = tmp.path().join("runs2.jsonl");
    write_n_records(&history_file2, 5, 80.0);
    let result_regress = ingest(&bench_file, &history_file2, 5, 3.0).unwrap();
    let acc_regress = result_regress.metrics.iter().find(|m| m.name == "accuracy_delta_pp").unwrap();
    assert_eq!(acc_regress.verdict, mqo_bench_history::types::Verdict::Regress);
}
