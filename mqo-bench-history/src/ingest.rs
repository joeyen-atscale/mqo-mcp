use crate::types::{AggMetrics, BenchReport, HistoryRecord, Verdict};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct MetricReport {
    pub name: &'static str,
    pub current: f64,
    pub baseline: f64,
    pub delta: f64,
    pub verdict: Verdict,
}

pub struct IngestResult {
    pub metrics: Vec<MetricReport>,
    pub has_baseline: bool,
    pub run_id: String,
    pub skipped_duplicate: bool,
}

fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn load_records(history_file: &Path) -> Result<Vec<HistoryRecord>, IngestError> {
    if !history_file.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(history_file)?;
    let records = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<HistoryRecord>(line).ok())
        .collect();
    Ok(records)
}

fn classify_accuracy(delta: f64, threshold: f64) -> Verdict {
    // For accuracy: negative delta = current is lower = regression
    if delta <= -threshold {
        Verdict::Regress
    } else if delta <= -2.0 {
        Verdict::Warn
    } else {
        Verdict::Ok
    }
}

fn classify_error_rate(delta: f64, threshold: f64) -> Verdict {
    // For entity_error_delta_pp: positive delta = error rate rose = regression
    if delta >= threshold {
        Verdict::Regress
    } else if delta >= 2.0 {
        Verdict::Warn
    } else {
        Verdict::Ok
    }
}

fn classify_increase(delta: f64, threshold: f64) -> Verdict {
    // For latency and tokens: increase = worse
    if delta >= threshold {
        Verdict::Regress
    } else if delta >= threshold * 0.4 {
        Verdict::Warn
    } else {
        Verdict::Ok
    }
}

pub fn ingest(
    bench_file: &Path,
    history_file: &Path,
    baseline_window: usize,
    regress_threshold: f64,
) -> Result<IngestResult, IngestError> {
    let bench_bytes = std::fs::read(bench_file)?;
    let report: BenchReport = serde_json::from_slice(&bench_bytes)?;
    let run_id = Uuid::new_v4().to_string();
    ingest_with_run_id(run_id, &bench_bytes, report, history_file, baseline_window, regress_threshold)
}

pub fn ingest_with_run_id(
    run_id: String,
    bench_bytes: &[u8],
    report: BenchReport,
    history_file: &Path,
    baseline_window: usize,
    regress_threshold: f64,
) -> Result<IngestResult, IngestError> {
    let task_file_hash = hash_bytes(bench_bytes);
    let timestamp = Utc::now().to_rfc3339();

    // Load existing records
    let existing_records = load_records(history_file)?;

    // Check for duplicate run_id
    if existing_records.iter().any(|r| r.run_id == run_id) {
        return Ok(IngestResult {
            metrics: vec![],
            has_baseline: false,
            run_id,
            skipped_duplicate: true,
        });
    }

    let per_question_count = report.per_question.len();
    let new_record = HistoryRecord {
        run_id: run_id.clone(),
        timestamp,
        aggregate: report.aggregate.clone(),
        per_question_count,
        task_file_hash,
    };

    // Create parent dir if needed
    if let Some(parent) = history_file.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }

    // Append new record
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(history_file)?;
    let line = serde_json::to_string(&new_record)?;
    writeln!(file, "{line}")?;

    // Need at least 2 prior records for a meaningful baseline
    if existing_records.len() < 2 {
        return Ok(IngestResult {
            metrics: vec![],
            has_baseline: false,
            run_id,
            skipped_duplicate: false,
        });
    }

    // Compute baseline from last N prior records
    let window = baseline_window.min(existing_records.len());
    let baseline_records = &existing_records[existing_records.len() - window..];

    let baseline = compute_baseline_mean(baseline_records);
    let current = &report.aggregate;

    let metrics = vec![
        MetricReport {
            name: "accuracy_delta_pp",
            current: current.accuracy_delta_pp,
            baseline: baseline.accuracy_delta_pp,
            delta: current.accuracy_delta_pp - baseline.accuracy_delta_pp,
            verdict: classify_accuracy(current.accuracy_delta_pp - baseline.accuracy_delta_pp, regress_threshold),
        },
        MetricReport {
            name: "entity_error_delta_pp",
            current: current.entity_error_delta_pp,
            baseline: baseline.entity_error_delta_pp,
            delta: current.entity_error_delta_pp - baseline.entity_error_delta_pp,
            verdict: classify_error_rate(current.entity_error_delta_pp - baseline.entity_error_delta_pp, regress_threshold),
        },
        MetricReport {
            name: "latency_delta_ms",
            current: current.latency_delta_ms,
            baseline: baseline.latency_delta_ms,
            delta: current.latency_delta_ms - baseline.latency_delta_ms,
            verdict: classify_increase(current.latency_delta_ms - baseline.latency_delta_ms, regress_threshold),
        },
        MetricReport {
            name: "token_delta",
            current: current.token_delta,
            baseline: baseline.token_delta,
            delta: current.token_delta - baseline.token_delta,
            verdict: classify_increase(current.token_delta - baseline.token_delta, regress_threshold),
        },
    ];

    Ok(IngestResult {
        metrics,
        has_baseline: true,
        run_id,
        skipped_duplicate: false,
    })
}

fn compute_baseline_mean(records: &[HistoryRecord]) -> AggMetrics {
    let n = records.len() as f64;
    AggMetrics {
        accuracy_delta_pp: records.iter().map(|r| r.aggregate.accuracy_delta_pp).sum::<f64>() / n,
        entity_error_delta_pp: records.iter().map(|r| r.aggregate.entity_error_delta_pp).sum::<f64>() / n,
        latency_delta_ms: records.iter().map(|r| r.aggregate.latency_delta_ms).sum::<f64>() / n,
        token_delta: records.iter().map(|r| r.aggregate.token_delta).sum::<f64>() / n,
    }
}

pub fn print_ingest_result(result: &IngestResult) {
    if result.skipped_duplicate {
        println!("run_id {} already exists — skipping", result.run_id);
        return;
    }

    if !result.has_baseline {
        println!("baseline: not enough runs");
        return;
    }

    println!("{:<25} {:>12} {:>12} {:>10}  verdict", "metric", "current", "baseline", "delta");
    println!("{}", "-".repeat(75));
    for m in &result.metrics {
        let arrow = if m.delta > 0.0 { "↑" } else if m.delta < 0.0 { "↓" } else { "→" };
        println!(
            "{:<25} {:>12.2} {:>12.2} {:>+10.2}{} {}",
            m.name, m.current, m.baseline, m.delta, arrow, m.verdict
        );
    }
}

pub fn has_regression(result: &IngestResult) -> bool {
    result.metrics.iter().any(|m| m.verdict == Verdict::Regress)
}
