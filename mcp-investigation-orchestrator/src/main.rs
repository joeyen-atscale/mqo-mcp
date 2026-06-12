#![deny(unsafe_code)]

use clap::Parser;
use mcp_concept_graph::ConceptGraph;
use mcp_finding_store::{Finding, FindingStatus, FindingStore};
use mcp_hypothesis_engine::{compute_delta, run_engine};
use mcp_query_budget_governor::{BudgetLedger, BudgetLimits, Verdict};
use serde_json::{json, Value};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Parser, Debug)]
#[command(name = "mcp-investigation-orchestrator", about = "Run one autonomous investigation pass")]
struct Args {
    #[arg(long)]
    watch_event: PathBuf,

    #[arg(long)]
    describe_model: PathBuf,

    #[arg(long)]
    finding_store: PathBuf,

    #[arg(long, default_value_t = 10)]
    budget_max_queries: u64,

    #[arg(long)]
    probe_executor: Option<PathBuf>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("mcp-investigation-orchestrator: fatal: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse();
    let now_ms = now_ms();

    let watch_event = read_json(&args.watch_event)?;
    let describe_model = read_json(&args.describe_model)?;
    let graph = ConceptGraph::from_describe_model(&describe_model)
        .map_err(|e| format!("cannot build concept graph from describe_model: {e}"))?;

    let target = event_measure(&watch_event).ok_or("watch-event missing measure name")?;
    if graph.node(&target).is_none() {
        return Err(format!("watch-event measure not found in describe_model graph: {target}"));
    }

    let target_delta = event_delta(&watch_event);
    let empty_summary = json!({});
    let hypset = run_engine(&graph, &target, target_delta, &empty_summary, &empty_summary, 4, 8);
    let hypset_value = serde_json::to_value(&hypset)
        .map_err(|e| format!("cannot serialize hypothesis set: {e}"))?;

    let probe_executor = match args.probe_executor {
        Some(path) => path,
        None => find_probe_executor().ok_or("cannot find mcp-probe-executor on PATH or ~/.local/bin")?,
    };

    let mut ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: Some(args.budget_max_queries),
            max_est_tokens: None,
            max_latency_ms: None,
            max_wall_ms: None,
            checkin_fraction: 0.8,
        },
        now_ms,
    );

    let mut resolved = Vec::new();
    let mut checkin_pending = false;
    let mut halted = false;
    let mut notes = Vec::new();

    let hypotheses = hypset_value
        .get("hypotheses")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    for hypothesis in hypotheses {
        match ledger.check(now_ms) {
            Verdict::Proceed => {}
            Verdict::CheckIn { reason, .. } => {
                checkin_pending = true;
                notes.push(format!("budget_checkin: {reason}"));
            }
            Verdict::Halt { reason, limit } => {
                halted = true;
                notes.push(format!("budget_halt: {reason}; limit={limit}"));
                break;
            }
        }

        let single_hypset = single_probe_hypset(&hypset_value, &hypothesis, target_delta)?;
        let output = run_probe_executor(&probe_executor, &single_hypset, now_ms)?;
        ledger.record_query(100, 1);
        resolved.extend(extract_resolved(&output));
    }

    if halted && resolved.is_empty() {
        notes.push("budget_halt_before_first_probe".to_string());
    }

    let status = status_for_resolved(&resolved);
    let resolved_set = json!({
        "target": hypset.target,
        "evidence_type": "structural",
        "analysis_note": "Orchestrator executed hypothesis probes under budget and recorded the resolved investigation.",
        "confirmed_count": resolved.iter().filter(|r| r.get("verdict").and_then(Value::as_str) == Some("confirmed")).count(),
        "refuted_count": resolved.iter().filter(|r| r.get("verdict").and_then(Value::as_str) == Some("refuted")).count(),
        "checkin_pending": checkin_pending,
        "halted": halted,
        "notes": notes,
        "resolved": resolved,
        "budget": {
            "queries_run": ledger.queries_run,
            "max_queries": args.budget_max_queries
        }
    });

    let store = FindingStore::open(&args.finding_store)
        .map_err(|e| format!("cannot open finding store: {e}"))?;
    let query_id = query_id(&watch_event, &target);
    let finding_id = store
        .record(&query_id, &watch_event, &resolved_set, status, now_ms)
        .map_err(|e| format!("cannot record finding: {e}"))?;
    let finding = store
        .get(&finding_id)
        .map_err(|e| format!("cannot read recorded finding: {e}"))?
        .ok_or_else(|| format!("recorded finding not found: {finding_id}"))?;

    println!(
        "{}",
        serde_json::to_string_pretty(&vec![finding])
            .map_err(|e| format!("cannot serialize findings: {e}"))?
    );
    Ok(())
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("cannot parse {} as JSON: {e}", path.display()))
}

fn event_measure(event: &Value) -> Option<String> {
    event
        .get("measure")
        .and_then(Value::as_str)
        .or_else(|| {
            event
                .get("query")
                .and_then(|q| q.get("measures"))
                .and_then(Value::as_array)
                .and_then(|m| m.first())
                .and_then(|m| m.get("unique_name"))
                .and_then(Value::as_str)
        })
        .map(ToOwned::to_owned)
}

fn event_delta(event: &Value) -> f64 {
    let observed = event.get("observed").and_then(Value::as_f64).unwrap_or(0.0);
    let prior = event.get("prior").and_then(Value::as_f64).unwrap_or(0.0);
    compute_delta(prior, observed)
}

fn query_id(event: &Value, target: &str) -> String {
    event
        .get("query_id")
        .and_then(Value::as_str)
        .or_else(|| event.get("id").and_then(Value::as_str))
        .or_else(|| {
            event
                .get("query")
                .and_then(|q| q.get("query_id"))
                .and_then(Value::as_str)
        })
        .unwrap_or(target)
        .to_string()
}

fn single_probe_hypset(hypset: &Value, hypothesis: &Value, target_delta: f64) -> Result<Value, String> {
    let mut h = hypothesis.clone();
    let obj = h
        .as_object_mut()
        .ok_or("hypothesis-engine emitted non-object hypothesis")?;
    obj.insert("predicted_direction".to_string(), json!(direction(target_delta)));
    obj.entry("component_delta_fraction".to_string())
        .or_insert_with(|| json!(target_delta));
    obj.insert("probe_key".to_string(), json!(probe_key(hypothesis)));

    Ok(json!({
        "target": hypset.get("target").cloned().unwrap_or(Value::Null),
        "hypotheses": [h]
    }))
}

fn direction(delta: f64) -> &'static str {
    if delta < 0.0 {
        "down"
    } else {
        "up"
    }
}

fn probe_key(hypothesis: &Value) -> String {
    hypothesis
        .get("probe_mqo")
        .and_then(|mqo| mqo.get("measures"))
        .and_then(Value::as_array)
        .and_then(|measures| measures.first())
        .and_then(|m| m.get("unique_name"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| hypothesis.get("rank").and_then(Value::as_u64).map_or("probe", |_| "probe"))
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn run_probe_executor(probe_executor: &Path, hypset: &Value, now_ms: u64) -> Result<Value, String> {
    let temp_dir = std::env::temp_dir();
    let nonce = format!("{}-{}", std::process::id(), now_ms);
    let hyp_path = temp_dir.join(format!("mcp-investigation-orchestrator-hypotheses-{nonce}.json"));
    let budget_path = temp_dir.join(format!("mcp-investigation-orchestrator-budget-{nonce}.json"));

    fs::write(
        &hyp_path,
        serde_json::to_vec(hypset).map_err(|e| format!("cannot encode hypotheses JSON: {e}"))?,
    )
    .map_err(|e| format!("cannot write temp hypotheses file: {e}"))?;
    fs::write(
        &budget_path,
        br#"{"max_queries":1,"checkin_fraction":0.8}"#,
    )
    .map_err(|e| format!("cannot write temp budget file: {e}"))?;

    let output = Command::new(probe_executor)
        .arg("--hypotheses")
        .arg(&hyp_path)
        .arg("--budget")
        .arg(&budget_path)
        .arg("--now-ms")
        .arg(now_ms.to_string())
        .arg("--format")
        .arg("json")
        .output()
        .map_err(|e| format!("cannot run probe executor {}: {e}", probe_executor.display()))?;

    let _ = fs::remove_file(&hyp_path);
    let _ = fs::remove_file(&budget_path);

    if !output.status.success() {
        return Err(format!(
            "probe executor failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("probe executor emitted invalid JSON: {e}; stdout={}", String::from_utf8_lossy(&output.stdout)))
}

fn extract_resolved(output: &Value) -> Vec<Value> {
    if let Some(resolved) = output.get("resolved").and_then(Value::as_array) {
        return resolved.clone();
    }
    if let Some(array) = output.as_array() {
        return array.clone();
    }
    vec![output.clone()]
}

fn status_for_resolved(resolved: &[Value]) -> FindingStatus {
    if resolved
        .iter()
        .any(|r| r.get("verdict").and_then(Value::as_str) == Some("confirmed"))
    {
        FindingStatus::Confirmed
    } else if resolved
        .iter()
        .any(|r| r.get("verdict").and_then(Value::as_str) == Some("refuted"))
    {
        FindingStatus::Refuted
    } else {
        FindingStatus::Open
    }
}

fn find_probe_executor() -> Option<PathBuf> {
    find_on_path("mcp-probe-executor").or_else(|| {
        std::env::var_os("HOME").and_then(|home| {
            let candidate = PathBuf::from(home).join(".local/bin/mcp-probe-executor");
            candidate.exists().then_some(candidate)
        })
    })
}

fn find_on_path(binary: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.exists())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[allow(dead_code)]
fn _assert_finding_serializable(_: &Finding) -> io::Result<()> {
    Ok(())
}
