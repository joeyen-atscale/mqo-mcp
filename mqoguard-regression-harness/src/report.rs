//! Report builder: aggregates per-record scores into per-mode metrics and
//! assembles the final `Report` struct. Also handles human-readable output.
//!
//! The `usize`→`f64` casts throughout this module are for task/rollout counts
//! that will never approach the f64 mantissa limit (52-bit); we suppress
//! the lint here rather than adding noise to every arithmetic expression.
#![allow(clippy::cast_precision_loss)]

use std::collections::HashMap;

use crate::score::{bucket_reason, score_record};
use crate::types::{
    Baseline, Corpus, ModeScore, OverallScore, RecordScore, Report, TrajectoryRecord,
};

/// Build the full report from corpus, records, baseline, and tolerance.
#[must_use]
pub fn build_report(
    corpus: &Corpus,
    records: &[TrajectoryRecord],
    baseline: &Baseline,
    tolerance: f64,
) -> Report {
    // Index tasks by ID.
    let task_map: HashMap<&str, &crate::types::Task> =
        corpus.tasks.iter().map(|t| (t.id.as_str(), t)).collect();

    // Score every record that maps to a known task.
    let scored: Vec<RecordScore> = records
        .iter()
        .filter_map(|rec| {
            let task = task_map.get(rec.task_id.as_str())?;
            Some(score_record(rec, task))
        })
        .collect();

    // Group by task_id → list of rollout scores.
    // Key = (task_id, mcp) to mirror Python's per-cell grouping.
    let mut by_task_cell: HashMap<(String, String), Vec<&RecordScore>> = HashMap::new();
    for s in &scored {
        let mcp = s.mcp.clone().unwrap_or_else(|| "unknown".to_owned());
        by_task_cell
            .entry((s.task_id.clone(), mcp))
            .or_default()
            .push(s);
    }

    // Determine failure_mode for each task.
    let mode_of: HashMap<&str, &str> = corpus
        .tasks
        .iter()
        .map(|t| {
            (
                t.id.as_str(),
                t.failure_mode.as_deref().unwrap_or("unknown"),
            )
        })
        .collect();

    // Aggregate per-mode.
    let mut mode_task_cells: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for (task_id, mcp) in by_task_cell.keys() {
        let mode = mode_of
            .get(task_id.as_str())
            .copied()
            .unwrap_or("unknown")
            .to_owned();
        mode_task_cells
            .entry(mode)
            .or_default()
            .push((task_id.clone(), mcp.clone()));
    }

    let mut mode_scores: Vec<ModeScore> = mode_task_cells
        .iter()
        .map(|(mode, cells)| {
            build_mode_score(mode, cells, &by_task_cell, baseline, tolerance)
        })
        .collect();

    // Sort modes deterministically.
    mode_scores.sort_by(|a, b| a.mode.cmp(&b.mode));

    // Overall aggregate (weighted by n_tasks per mode).
    let n_tasks = by_task_cell.len();
    let n_records = scored.len();

    let (overall_path_mean, overall_pass_at_k) = {
        let total: usize = mode_scores.iter().map(|m| m.n_tasks).sum();
        if total == 0 {
            (0.0_f64, 0.0_f64)
        } else {
            let sum_pm: f64 = mode_scores
                .iter()
                .map(|m| m.path_mean * m.n_tasks as f64)
                .sum();
            let sum_pak: f64 = mode_scores
                .iter()
                .map(|m| m.pass_at_k * m.n_tasks as f64)
                .sum();
            (sum_pm / total as f64, sum_pak / total as f64)
        }
    };

    let failing_modes: Vec<String> = mode_scores
        .iter()
        .filter(|m| m.is_gated && !m.path_mean_ok)
        .map(|m| m.mode.clone())
        .collect();

    // Check overall floor if specified.
    let overall_floor_fail = baseline
        .overall_path_mean_floor
        .is_some_and(|f| overall_path_mean < f - tolerance);

    let any_below_floor = !failing_modes.is_empty() || overall_floor_fail;

    let overall = OverallScore {
        n_tasks,
        n_records,
        path_mean: overall_path_mean,
        pass_at_k: overall_pass_at_k,
        any_below_floor,
        failing_modes,
    };

    Report {
        modes: mode_scores,
        overall,
        tolerance,
    }
}

fn build_mode_score(
    mode: &str,
    cells: &[(String, String)],
    by_task_cell: &HashMap<(String, String), Vec<&RecordScore>>,
    baseline: &Baseline,
    tolerance: f64,
) -> ModeScore {
    let mut task_pass_path: Vec<f64> = Vec::new();
    let mut task_pass_at_k: Vec<f64> = Vec::new();
    let mut k_per_task: Vec<usize> = Vec::new();
    let mut failure_reasons: HashMap<String, usize> = HashMap::new();

    for cell in cells {
        let rollouts = by_task_cell.get(cell).map_or(&[] as &[&RecordScore], Vec::as_slice);
        let k = rollouts.len();
        if k == 0 {
            continue;
        }
        let n_pass = rollouts.iter().filter(|r| r.pass_by_path).count();
        task_pass_path.push(n_pass as f64 / k as f64);
        task_pass_at_k.push(if n_pass > 0 { 1.0 } else { 0.0 });
        k_per_task.push(k);

        // Count failure reasons.
        for r in rollouts {
            if !r.pass_by_path {
                let bucket = bucket_reason(&r.why_path).to_owned();
                *failure_reasons.entry(bucket).or_insert(0) += 1;
            }
        }
    }

    let n_tasks = task_pass_path.len();
    let path_mean = if n_tasks == 0 {
        0.0
    } else {
        task_pass_path.iter().sum::<f64>() / n_tasks as f64
    };
    let pass_at_k = if n_tasks == 0 {
        0.0
    } else {
        task_pass_at_k.iter().sum::<f64>() / n_tasks as f64
    };
    let avg_k = if k_per_task.is_empty() {
        0.0
    } else {
        k_per_task.iter().sum::<usize>() as f64 / k_per_task.len() as f64
    };

    // Gate fields from baseline.
    let floor_entry = baseline.modes.get(mode);
    let path_mean_floor = floor_entry.and_then(|e| e.path_mean_floor);
    let pass_at_k_floor = floor_entry.and_then(|e| e.pass_at_k_floor);
    let is_gated = floor_entry.is_some();

    let effective_floor = path_mean_floor.map(|f| f - tolerance);
    let path_mean_ok = effective_floor.is_none_or(|f| path_mean >= f);
    let path_mean_delta = path_mean_floor.map(|f| path_mean - f);

    ModeScore {
        mode: mode.to_owned(),
        n_tasks,
        path_mean,
        pass_at_k,
        avg_k,
        failure_reasons,
        path_mean_floor,
        pass_at_k_floor,
        is_gated,
        path_mean_ok,
        path_mean_delta,
    }
}

/// Print a human-readable report table to stdout.
pub fn print_human(report: &Report) {
    // Header.
    println!(
        "\n{:<30} {:>6}  {:>10}  {:>10}  {:>8}  {:>6}  {:>8}  status",
        "mode", "tasks", "path-mean", "pass@k", "floor", "gated", "delta"
    );
    println!("{}", "-".repeat(100));

    for m in &report.modes {
        let floor_str = m
            .path_mean_floor
            .map_or_else(|| "-".to_owned(), |f| format!("{:.1}%", f * 100.0));
        let gated_str = if m.is_gated { "yes" } else { "no" };
        let delta_str = m
            .path_mean_delta
            .map_or_else(|| "-".to_owned(), |d| format!("{:+.1}%", d * 100.0));
        let status = if !m.is_gated {
            "ungated"
        } else if m.path_mean_ok {
            "PASS"
        } else {
            "FAIL"
        };
        println!(
            "{:<30} {:>6}  {:>9.1}%  {:>9.1}%  {:>8}  {:>6}  {:>8}  {}",
            m.mode,
            m.n_tasks,
            m.path_mean * 100.0,
            m.pass_at_k * 100.0,
            floor_str,
            gated_str,
            delta_str,
            status,
        );
    }

    println!("{}", "-".repeat(100));
    println!(
        "{:<30} {:>6}  {:>9.1}%  {:>9.1}%",
        "OVERALL",
        report.overall.n_tasks,
        report.overall.path_mean * 100.0,
        report.overall.pass_at_k * 100.0,
    );

    if report.tolerance > 0.0 {
        println!(
            "\ntolerance: {:.1}% applied to floors",
            report.tolerance * 100.0
        );
    }

    println!();

    if report.overall.any_below_floor {
        println!(
            "REGRESSION: {} mode(s) below floor: {}",
            report.overall.failing_modes.len(),
            report.overall.failing_modes.join(", ")
        );
        // Print failure reason breakdown for failing modes.
        for m in &report.modes {
            if m.is_gated && !m.path_mean_ok && !m.failure_reasons.is_empty() {
                println!("\n  failure reasons for {}:", m.mode);
                let mut reasons: Vec<(&String, &usize)> = m.failure_reasons.iter().collect();
                reasons.sort_by(|a, b| b.1.cmp(a.1));
                for (reason, count) in reasons.iter().take(5) {
                    println!("    {reason:<40} {count}");
                }
            }
        }
    } else {
        println!("OK: all gated modes meet their floors.");
    }
}
