use mqo_bench_history::ingest::{has_regression, ingest};
use mqo_bench_history::types::{AggMetrics, HistoryRecord, Verdict};
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
            run_id: format!("prior-run-{}", i),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            aggregate: AggMetrics {
                accuracy_delta_pp: accuracy,
                entity_error_delta_pp: -5.0,
                latency_delta_ms: -100.0,
                token_delta: -50.0,
            },
            per_question_count: 10,
            task_file_hash: "abc123".to_string(),
        };
        writeln!(file, "{}", serde_json::to_string(&record).unwrap()).unwrap();
    }
}

#[test]
fn ac3_regress_when_accuracy_drops_more_than_threshold() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("runs.jsonl");

    // 5 prior records with baseline accuracy=80.0
    write_n_records(&history_file, 5, 80.0);

    // Current accuracy = 73.0 — drops 7 pp (> 5 pp threshold) → REGRESS
    fs::write(&bench_file, make_bench_json(73.0)).unwrap();
    let result = ingest(&bench_file, &history_file, 5, 5.0).unwrap();
    assert!(result.has_baseline);

    let acc_metric = result.metrics.iter().find(|m| m.name == "accuracy_delta_pp").unwrap();
    assert_eq!(acc_metric.verdict, Verdict::Regress, "7 pp drop should be REGRESS");
    assert!(has_regression(&result));
}

#[test]
fn ac3_warn_when_accuracy_drops_2_to_5_pp() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("runs.jsonl");

    write_n_records(&history_file, 5, 80.0);

    // Current accuracy = 76.5 — drops 3.5 pp (> 2 pp, < 5 pp) → WARN
    fs::write(&bench_file, make_bench_json(76.5)).unwrap();
    let result = ingest(&bench_file, &history_file, 5, 5.0).unwrap();
    assert!(result.has_baseline);

    let acc_metric = result.metrics.iter().find(|m| m.name == "accuracy_delta_pp").unwrap();
    assert_eq!(acc_metric.verdict, Verdict::Warn, "3.5 pp drop should be WARN");
    assert!(!has_regression(&result));
}

#[test]
fn ac3_ok_when_accuracy_within_threshold() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("runs.jsonl");

    write_n_records(&history_file, 5, 80.0);

    // Current accuracy = 79.5 — drops only 0.5 pp → OK
    fs::write(&bench_file, make_bench_json(79.5)).unwrap();
    let result = ingest(&bench_file, &history_file, 5, 5.0).unwrap();
    assert!(result.has_baseline);

    let acc_metric = result.metrics.iter().find(|m| m.name == "accuracy_delta_pp").unwrap();
    assert_eq!(acc_metric.verdict, Verdict::Ok, "0.5 pp drop should be OK");
    assert!(!has_regression(&result));
}
