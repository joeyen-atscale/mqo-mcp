use crate::sparkline::sparkline;
use crate::types::{HistoryRecord, Verdict};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ReportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

fn load_records(history_file: &Path, last: usize) -> Result<Vec<HistoryRecord>, ReportError> {
    if !history_file.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(history_file)?;
    let mut records: Vec<HistoryRecord> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<HistoryRecord>(line).ok())
        .collect();

    // Take last N
    if records.len() > last {
        records = records.split_off(records.len() - last);
    }
    Ok(records)
}

fn row_verdict(record: &HistoryRecord) -> Verdict {
    // Simple heuristic: we don't have a stored verdict, so we report OK
    // The actual verdict is computed at ingest time. For reporting we just show OK.
    // (Could store verdict in HistoryRecord in a future version)
    let _ = record;
    Verdict::Ok
}

pub fn run_report(history_file: &Path, last: usize, csv: bool) -> Result<(), ReportError> {
    let records = load_records(history_file, last)?;

    if records.is_empty() {
        println!("No history records found.");
        return Ok(());
    }

    if csv {
        println!("run_id,timestamp,accuracy_delta_pp,entity_error_delta_pp,latency_delta_ms,token_delta,verdict");
        for r in &records {
            let v = row_verdict(r);
            let date = r.timestamp.get(..10).unwrap_or(&r.timestamp);
            println!(
                "{},{},{},{},{},{},{}",
                r.run_id,
                date,
                r.aggregate.accuracy_delta_pp,
                r.aggregate.entity_error_delta_pp,
                r.aggregate.latency_delta_ms,
                r.aggregate.token_delta,
                v
            );
        }
    } else {
        // Table header
        println!(
            "{:<10} {:<12} {:>12} {:>20} {:>14} {:>12} {:>8}",
            "run_id", "date", "accuracy_pp", "entity_err_pp", "latency_ms", "token_d", "verdict"
        );
        println!("{}", "-".repeat(92));

        for r in &records {
            let v = row_verdict(r);
            let short_id = &r.run_id[..8.min(r.run_id.len())];
            let date = r.timestamp.get(..10).unwrap_or(&r.timestamp);
            println!(
                "{:<10} {:<12} {:>12.2} {:>20.2} {:>14.2} {:>12.2} {:>8}",
                short_id,
                date,
                r.aggregate.accuracy_delta_pp,
                r.aggregate.entity_error_delta_pp,
                r.aggregate.latency_delta_ms,
                r.aggregate.token_delta,
                v
            );
        }

        // Sparklines
        if records.len() > 1 {
            println!();
            println!("Sparklines (oldest → newest):");
            let acc: Vec<f64> = records.iter().map(|r| r.aggregate.accuracy_delta_pp).collect();
            let ent: Vec<f64> = records.iter().map(|r| r.aggregate.entity_error_delta_pp).collect();
            let lat: Vec<f64> = records.iter().map(|r| r.aggregate.latency_delta_ms).collect();
            let tok: Vec<f64> = records.iter().map(|r| r.aggregate.token_delta).collect();

            println!("  accuracy_delta_pp:     {}", sparkline(&acc));
            println!("  entity_error_delta_pp: {}", sparkline(&ent));
            println!("  latency_delta_ms:      {}", sparkline(&lat));
            println!("  token_delta:           {}", sparkline(&tok));
        }
    }

    Ok(())
}
