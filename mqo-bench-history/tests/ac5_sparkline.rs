use mqo_bench_history::sparkline::sparkline;

const BLOCK_CHARS: &str = "▁▂▃▄▅▆▇█";

fn is_block_char(c: char) -> bool {
    BLOCK_CHARS.contains(c)
}

#[test]
fn ac5_sparkline_contains_block_chars_for_multiple_values() {
    let values = vec![70.0, 75.0, 80.0];
    let s = sparkline(&values);
    assert!(
        s.chars().all(is_block_char),
        "All chars in sparkline should be block chars, got: {}",
        s
    );
    assert_eq!(s.chars().count(), 3);
}

#[test]
fn ac5_single_value_returns_flat() {
    let s = sparkline(&[42.0]);
    assert_eq!(s, "▄");
}

#[test]
fn ac5_empty_returns_empty() {
    let s = sparkline(&[]);
    assert!(s.is_empty());
}

#[test]
fn ac5_min_max_extremes() {
    let s = sparkline(&[0.0, 1.0]);
    let chars: Vec<char> = s.chars().collect();
    assert_eq!(chars.len(), 2);
    assert_eq!(chars[0], '▁'); // min
    assert_eq!(chars[1], '█'); // max
}

#[test]
fn ac5_all_equal_returns_mid_block() {
    let s = sparkline(&[50.0, 50.0, 50.0, 50.0]);
    assert!(s.chars().all(|c| c == '▄'));
}

#[test]
fn ac5_report_output_contains_sparkline_chars() {
    use mqo_bench_history::report::run_report;
    use mqo_bench_history::types::{AggMetrics, HistoryRecord};
    use std::io::Write;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let history_file = tmp.path().join("runs.jsonl");

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&history_file)
        .unwrap();

    for i in 0..3 {
        let r = HistoryRecord {
            run_id: format!("run-{}", i),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            aggregate: AggMetrics {
                accuracy_delta_pp: 70.0 + i as f64 * 5.0,
                entity_error_delta_pp: -5.0,
                latency_delta_ms: -100.0,
                token_delta: -50.0,
            },
            per_question_count: 5,
            task_file_hash: "hash".to_string(),
        };
        writeln!(file, "{}", serde_json::to_string(&r).unwrap()).unwrap();
    }

    // run_report with 3 records — it should print sparklines (len>1)
    // We just verify it completes without error; sparklines are printed to stdout
    run_report(&history_file, 10, false).expect("should succeed");
    // The actual char verification happens in ac8 via process::Command
}
