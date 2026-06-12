use clap::Parser;
use mcp_query_budget_governor::{BudgetLedger, BudgetLimits, Verdict};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

// ── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(name = "mcp-probe-executor", about = "Execute hypothesis probes under budget")]
struct Args {
    /// HypothesisSet JSON from mcp-hypothesis-engine
    #[arg(long)]
    hypotheses: PathBuf,

    /// BudgetLimits JSON for mcp-query-budget-governor
    #[arg(long)]
    budget: PathBuf,

    /// TEST MODE: directory containing <probe_key>.json current DatasetSummary files
    #[arg(long)]
    summaries: Option<PathBuf>,

    /// Baseline summaries directory to delta against
    #[arg(long)]
    baseline: Option<PathBuf>,

    /// Budget clock timestamp in ms (also enables determinism)
    #[arg(long)]
    now_ms: Option<u64>,

    /// Minimum absolute delta fraction to call "confirmed"
    #[arg(long, default_value = "0.02")]
    confirm_min_fraction: f64,

    /// Output format: json or human
    #[arg(long, default_value = "json")]
    format: String,
}

// ── Input types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize, Clone)]
struct ProbeMqo {
    measures: Value,
    dimensions: Value,
    filters: Value,
}

#[derive(Debug, Deserialize, Clone)]
struct Hypothesis {
    rank: u32,
    explanation: String,
    probe_mqo: ProbeMqo,
    predicted_direction: String,
    component_delta_fraction: f64,
    probe_key: String,
}

#[derive(Debug, Deserialize)]
struct HypothesisSet {
    target: String,
    hypotheses: Vec<Hypothesis>,
}

#[derive(Debug, Deserialize)]
struct BudgetLimitsJson {
    max_queries: Option<u64>,
    checkin_fraction: Option<f64>,
    max_tokens: Option<u64>,
    max_est_tokens: Option<u64>,
    window_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct DatasetSummary {
    mean: f64,
    #[allow(dead_code)]
    row_count: Option<u64>,
}

// ── Output types ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct ResolvedHypothesis {
    rank: u32,
    explanation: String,
    probe_mqo: Value,
    predicted_direction: String,
    observed_delta_fraction: Option<f64>,
    verdict: String,
}

#[derive(Debug, Serialize)]
struct BudgetSummary {
    queries_run: u64,
    verdict_at_stop: String,
}

#[derive(Debug, Serialize)]
struct ResolvedHypothesisSet {
    target: String,
    evidence_type: String,
    analysis_note: String,
    confirmed_count: usize,
    refuted_count: usize,
    checkin_pending: bool,
    halted: bool,
    resolved: Vec<ResolvedHypothesis>,
    budget: BudgetSummary,
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn load_budget_limits(path: &PathBuf) -> anyhow_lite::Result<BudgetLimits> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read budget file {:?}: {}", path, e))?;
    let j: BudgetLimitsJson = serde_json::from_str(&text)
        .map_err(|e| format!("cannot parse budget JSON {:?}: {}", path, e))?;

    Ok(BudgetLimits {
        max_queries: j.max_queries,
        max_est_tokens: j.max_est_tokens.or(j.max_tokens),
        max_latency_ms: None,
        max_wall_ms: j.window_ms,
        checkin_fraction: j.checkin_fraction.unwrap_or(0.8),
    })
}

fn load_summary(dir: &PathBuf, probe_key: &str) -> Result<DatasetSummary, String> {
    let path = dir.join(format!("{}.json", probe_key));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read summary {:?}: {}", path, e))?;
    serde_json::from_str::<DatasetSummary>(&text)
        .map_err(|e| format!("cannot parse summary {:?}: {}", path, e))
}

fn verdict_label(v: &Verdict) -> &'static str {
    match v {
        Verdict::Proceed => "Proceed",
        Verdict::CheckIn { .. } => "CheckIn",
        Verdict::Halt { .. } => "Halt",
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────

// Lightweight error type to avoid pulling in anyhow
mod anyhow_lite {
    pub type Result<T> = std::result::Result<T, String>;
}

fn run() -> anyhow_lite::Result<()> {
    let args = Args::parse();

    // Load hypothesis set
    let hypset_text = std::fs::read_to_string(&args.hypotheses)
        .map_err(|e| format!("cannot read hypotheses file: {}", e))?;
    let hypset: HypothesisSet = serde_json::from_str(&hypset_text)
        .map_err(|e| format!("cannot parse hypotheses JSON: {}", e))?;

    // Load budget limits and construct ledger
    let limits = load_budget_limits(&args.budget)?;
    let now_ms = args.now_ms.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    });
    let mut ledger = BudgetLedger::new(limits, now_ms);

    // Process each hypothesis
    let mut resolved: Vec<ResolvedHypothesis> = Vec::new();
    let mut checkin_pending = false;
    let mut halted = false;
    let mut last_verdict_label = "Proceed";

    let mut hypotheses = hypset.hypotheses.clone();
    hypotheses.sort_by_key(|h| h.rank);

    for hyp in &hypotheses {
        // Check budget before executing
        let verdict = ledger.check(now_ms);
        last_verdict_label = verdict_label(&verdict);

        match verdict {
            Verdict::Halt { .. } => {
                // Mark this and all remaining as skipped_budget
                resolved.push(ResolvedHypothesis {
                    rank: hyp.rank,
                    explanation: hyp.explanation.clone(),
                    probe_mqo: serde_json::to_value(&hyp.probe_mqo).unwrap_or(Value::Null),
                    predicted_direction: hyp.predicted_direction.clone(),
                    observed_delta_fraction: None,
                    verdict: "skipped_budget".to_string(),
                });
                halted = true;
                continue;
            }
            Verdict::CheckIn { .. } => {
                checkin_pending = true;
                // continue executing
            }
            Verdict::Proceed => {}
        }

        if halted {
            // Already halted from a previous iteration (shouldn't happen with the logic above,
            // but guard defensively)
            resolved.push(ResolvedHypothesis {
                rank: hyp.rank,
                explanation: hyp.explanation.clone(),
                probe_mqo: serde_json::to_value(&hyp.probe_mqo).unwrap_or(Value::Null),
                predicted_direction: hyp.predicted_direction.clone(),
                observed_delta_fraction: None,
                verdict: "skipped_budget".to_string(),
            });
            continue;
        }

        // Execute probe
        let probe_result = execute_probe(&args, &hyp.probe_key);

        match probe_result {
            Err(e) => {
                eprintln!(
                    "mcp-probe-executor: inconclusive probe for key '{}': {}",
                    hyp.probe_key, e
                );
                resolved.push(ResolvedHypothesis {
                    rank: hyp.rank,
                    explanation: hyp.explanation.clone(),
                    probe_mqo: serde_json::to_value(&hyp.probe_mqo).unwrap_or(Value::Null),
                    predicted_direction: hyp.predicted_direction.clone(),
                    observed_delta_fraction: None,
                    verdict: "inconclusive".to_string(),
                });
                // Still record the query attempt
                ledger.record_query(100, 1);
            }
            Ok((mean_now, mean_base)) => {
                let delta = (mean_now - mean_base) / mean_base.abs();

                let verdict_str = compute_verdict(
                    delta,
                    &hyp.predicted_direction,
                    args.confirm_min_fraction,
                );

                resolved.push(ResolvedHypothesis {
                    rank: hyp.rank,
                    explanation: hyp.explanation.clone(),
                    probe_mqo: serde_json::to_value(&hyp.probe_mqo).unwrap_or(Value::Null),
                    predicted_direction: hyp.predicted_direction.clone(),
                    observed_delta_fraction: Some(delta),
                    verdict: verdict_str,
                });
                ledger.record_query(100, 1);
            }
        }
    }

    // After the loop ends — if we broke early due to Halt, remaining hypotheses were
    // already marked skipped_budget inside the loop. However the loop above doesn't break
    // on Halt; it marks each hypothesis individually. This is correct for the AC2 test.
    // But we need to handle the case where a Halt verdict is first seen at query N,
    // meaning hypotheses N, N+1, ... should be skipped. The current logic correctly marks
    // each such hypothesis as skipped_budget on the check() call. Good.

    let confirmed_count = resolved.iter().filter(|r| r.verdict == "confirmed").count();
    let refuted_count = resolved.iter().filter(|r| r.verdict == "refuted").count();

    let output = ResolvedHypothesisSet {
        target: hypset.target,
        evidence_type: "structural".to_string(),
        analysis_note: "Probes executed under budget; confirm/refute is a directional data check, not statistical causation.".to_string(),
        confirmed_count,
        refuted_count,
        checkin_pending,
        halted,
        resolved,
        budget: BudgetSummary {
            queries_run: ledger.queries_run,
            verdict_at_stop: last_verdict_label.to_string(),
        },
    };

    if args.format == "human" {
        print_human(&output);
    } else {
        println!("{}", serde_json::to_string_pretty(&output).unwrap());
    }

    Ok(())
}

fn execute_probe(args: &Args, probe_key: &str) -> Result<(f64, f64), String> {
    // Test mode: read from summaries dir
    if let Some(ref summaries_dir) = args.summaries {
        let current = load_summary(summaries_dir, probe_key)
            .map_err(|e| format!("current summary: {}", e))?;

        let baseline_dir = args
            .baseline
            .as_ref()
            .ok_or_else(|| format!("--baseline dir required in test mode for key '{}'", probe_key))?;
        let baseline = load_summary(baseline_dir, probe_key)
            .map_err(|e| format!("baseline summary: {}", e))?;

        return Ok((current.mean, baseline.mean));
    }

    Err(format!(
        "no execution mode configured for probe key '{}' (pass --summaries for test mode)",
        probe_key
    ))
}

fn compute_verdict(delta: f64, predicted_direction: &str, confirm_min_fraction: f64) -> String {
    let same_direction = match predicted_direction {
        "down" => delta < 0.0,
        "up" => delta > 0.0,
        _ => false,
    };

    if same_direction && delta.abs() >= confirm_min_fraction {
        "confirmed".to_string()
    } else {
        "refuted".to_string()
    }
}

fn print_human(output: &ResolvedHypothesisSet) {
    println!("Target: {}", output.target);
    println!("Evidence type: {}", output.evidence_type);
    println!("Note: {}", output.analysis_note);
    println!(
        "Confirmed: {}  Refuted: {}  Halted: {}  CheckIn pending: {}",
        output.confirmed_count, output.refuted_count, output.halted, output.checkin_pending
    );
    println!(
        "Budget: {} queries run, verdict at stop: {}",
        output.budget.queries_run, output.budget.verdict_at_stop
    );
    for r in &output.resolved {
        let delta_str = r
            .observed_delta_fraction
            .map(|d| format!("{:+.3}", d))
            .unwrap_or_else(|| "N/A".to_string());
        println!(
            "  [{}] rank={} predicted={} delta={} verdict={}",
            r.verdict, r.rank, r.predicted_direction, delta_str, r.verdict
        );
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("mcp-probe-executor: fatal: {}", e);
        std::process::exit(1);
    }
}
