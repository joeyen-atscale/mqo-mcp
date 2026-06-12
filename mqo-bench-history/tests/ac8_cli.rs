use std::fs;
use std::io::Write;
use std::process::Command;
use tempfile::TempDir;

fn binary_path() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("mqo-bench-history");
    path
}

fn make_bench_json(accuracy: f64) -> String {
    serde_json::json!({
        "aggregate": {
            "accuracy_delta_pp": accuracy,
            "entity_error_delta_pp": -5.0,
            "latency_delta_ms": -100.0,
            "token_delta": -50.0
        },
        "per_question": [{"q": "1"}, {"q": "2"}]
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
        let record = serde_json::json!({
            "run_id": format!("cli-prior-{}", i),
            "timestamp": "2026-01-01T00:00:00Z",
            "aggregate": {
                "accuracy_delta_pp": accuracy,
                "entity_error_delta_pp": -5.0,
                "latency_delta_ms": -100.0,
                "token_delta": -50.0
            },
            "per_question_count": 10,
            "task_file_hash": "abc123"
        });
        writeln!(file, "{}", record).unwrap();
    }
}

#[test]
fn ac8_ingest_exits_zero_when_no_regression() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("runs.jsonl");

    fs::write(&bench_file, make_bench_json(80.0)).unwrap();

    let bin = binary_path();
    let status = Command::new(&bin)
        .args([
            "ingest",
            bench_file.to_str().unwrap(),
            "--history-file",
            history_file.to_str().unwrap(),
        ])
        .status()
        .expect("binary should run");

    assert!(status.success(), "Exit 0 expected when no baseline exists");
}

#[test]
fn ac8_ingest_exits_one_on_regression() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("runs.jsonl");

    // 5 prior records with baseline=80
    write_n_records(&history_file, 5, 80.0);

    // Current = 72.0 → 8 pp drop → REGRESS → exit 1
    fs::write(&bench_file, make_bench_json(72.0)).unwrap();

    let bin = binary_path();
    let status = Command::new(&bin)
        .args([
            "ingest",
            bench_file.to_str().unwrap(),
            "--history-file",
            history_file.to_str().unwrap(),
        ])
        .status()
        .expect("binary should run");

    assert_eq!(status.code(), Some(1), "Exit 1 expected when regression detected");
}

#[test]
fn ac8_report_csv_has_correct_header() {
    let tmp = TempDir::new().unwrap();
    let history_file = tmp.path().join("runs.jsonl");

    write_n_records(&history_file, 3, 80.0);

    let bin = binary_path();
    let output = Command::new(&bin)
        .args([
            "report",
            "--csv",
            "--history-file",
            history_file.to_str().unwrap(),
        ])
        .output()
        .expect("binary should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_line = stdout.lines().next().unwrap_or("");
    assert_eq!(
        first_line,
        "run_id,timestamp,accuracy_delta_pp,entity_error_delta_pp,latency_delta_ms,token_delta,verdict"
    );
}

#[test]
fn ac8_report_csv_has_correct_row_count() {
    let tmp = TempDir::new().unwrap();
    let history_file = tmp.path().join("runs.jsonl");

    write_n_records(&history_file, 5, 80.0);

    let bin = binary_path();
    let output = Command::new(&bin)
        .args([
            "report",
            "--csv",
            "--last",
            "5",
            "--history-file",
            history_file.to_str().unwrap(),
        ])
        .output()
        .expect("binary should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    // header + 5 data rows
    assert_eq!(lines.len(), 6, "Expected header + 5 data rows, got: {:?}", lines);
}

#[test]
fn ac8_report_table_contains_sparklines() {
    let tmp = TempDir::new().unwrap();
    let history_file = tmp.path().join("runs.jsonl");

    write_n_records(&history_file, 3, 80.0);

    let bin = binary_path();
    let output = Command::new(&bin)
        .args([
            "report",
            "--history-file",
            history_file.to_str().unwrap(),
        ])
        .output()
        .expect("binary should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let sparkline_chars = "▁▂▃▄▅▆▇█";
    assert!(
        stdout.chars().any(|c| sparkline_chars.contains(c)),
        "Report output should contain sparkline characters. Got:\n{}",
        stdout
    );
}

#[test]
fn ac8_ingest_baseline_not_enough_runs_message() {
    let tmp = TempDir::new().unwrap();
    let bench_file = tmp.path().join("bench.json");
    let history_file = tmp.path().join("runs.jsonl");

    fs::write(&bench_file, make_bench_json(80.0)).unwrap();

    let bin = binary_path();
    let output = Command::new(&bin)
        .args([
            "ingest",
            bench_file.to_str().unwrap(),
            "--history-file",
            history_file.to_str().unwrap(),
        ])
        .output()
        .expect("binary should run");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("not enough runs"),
        "Should print 'not enough runs' for first ingest. Got: {}",
        stdout
    );
}
