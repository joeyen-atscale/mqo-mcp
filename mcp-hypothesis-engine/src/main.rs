// mcp-hypothesis-engine — Gen-9 binary entry point
//
// Fuses mcp-causal-tracer (structural derivation paths) and
// mcp-next-query-proposer (probe MQOs) into one autonomous step.
// No network, no LLM.

use std::path::PathBuf;
use std::process;

use clap::Parser;
use mcp_concept_graph::ConceptGraph;
use mcp_hypothesis_engine::engine::{compute_delta, extract_mean, run_engine, HypothesisSet, Corroboration};
use serde::Deserialize;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "mcp-hypothesis-engine", version, about)]
struct Cli {
    /// Path to concept graph JSON (produced by ConceptGraph::to_json())
    #[arg(long)]
    graph: PathBuf,

    /// Measure name that moved (mutually exclusive with --from-event)
    #[arg(long)]
    target: Option<String>,

    /// WatchEvent JSON — supplies target + observed/prior
    #[arg(long)]
    from_event: Option<PathBuf>,

    /// Baseline DatasetSummary JSON
    #[arg(long)]
    handle_a: PathBuf,

    /// Current DatasetSummary JSON
    #[arg(long)]
    handle_b: PathBuf,

    /// BFS depth limit (default 4)
    #[arg(long, default_value_t = 4)]
    max_depth: u8,

    /// Number of hypotheses to emit (default 8)
    #[arg(long, default_value_t = 8)]
    top_k: usize,

    /// Output format: json (default) or human
    #[arg(long, default_value = "json")]
    format: String,
}

// ---------------------------------------------------------------------------
// WatchEvent
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct WatchEvent {
    measure: Option<String>,
    observed: Option<f64>,
    prior: Option<f64>,
    query: Option<WatchQuery>,
}

#[derive(Debug, Deserialize)]
struct WatchQuery {
    measures: Option<Vec<WatchMeasure>>,
}

#[derive(Debug, Deserialize)]
struct WatchMeasure {
    unique_name: Option<String>,
}

// ---------------------------------------------------------------------------
// Error output helper
// ---------------------------------------------------------------------------

fn error_exit(msg: &str) -> ! {
    eprintln!("{}", json!({"error": msg}));
    process::exit(1);
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();

    // Load graph
    let graph_json: Value = {
        let raw = std::fs::read_to_string(&cli.graph)
            .unwrap_or_else(|e| error_exit(&format!("cannot read graph file: {e}")));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| error_exit(&format!("invalid graph JSON: {e}")))
    };
    let graph = ConceptGraph::from_json(&graph_json);

    // Load summaries
    let summary_a: Value = {
        let raw = std::fs::read_to_string(&cli.handle_a)
            .unwrap_or_else(|e| error_exit(&format!("cannot read handle-a: {e}")));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| error_exit(&format!("invalid handle-a JSON: {e}")))
    };
    let summary_b: Value = {
        let raw = std::fs::read_to_string(&cli.handle_b)
            .unwrap_or_else(|e| error_exit(&format!("cannot read handle-b: {e}")));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| error_exit(&format!("invalid handle-b JSON: {e}")))
    };

    // Resolve target + target delta
    let (target, target_delta) = if let Some(event_path) = &cli.from_event {
        let raw = std::fs::read_to_string(event_path)
            .unwrap_or_else(|e| error_exit(&format!("cannot read from-event: {e}")));
        let event: WatchEvent = serde_json::from_str(&raw)
            .unwrap_or_else(|e| error_exit(&format!("invalid watch-event JSON: {e}")));

        let measure = event
            .measure
            .clone()
            .or_else(|| {
                event
                    .query
                    .as_ref()
                    .and_then(|q| q.measures.as_ref())
                    .and_then(|ms| ms.first())
                    .and_then(|m| m.unique_name.clone())
            })
            .unwrap_or_else(|| error_exit("watch-event missing measure name"));

        let observed = event.observed.unwrap_or(0.0);
        let prior = event.prior.unwrap_or(0.0);
        let delta = compute_delta(prior, observed);

        (measure, delta)
    } else {
        let t = cli
            .target
            .clone()
            .unwrap_or_else(|| error_exit("either --target or --from-event is required"));

        let mean_a = extract_mean(&summary_a, &t);
        let mean_b = extract_mean(&summary_b, &t);
        let delta = match (mean_a, mean_b) {
            (Some(a), Some(b)) => compute_delta(a, b),
            _ => 0.0,
        };
        (t, delta)
    };

    // Check target exists in graph
    if graph.node(&target).is_none() {
        eprintln!("{}", json!({"error": "target not found in concept graph"}));
        process::exit(1);
    }

    // Run engine
    let result = run_engine(
        &graph,
        &target,
        target_delta,
        &summary_a,
        &summary_b,
        cli.max_depth,
        cli.top_k,
    );

    // Emit
    match cli.format.as_str() {
        "human" => emit_human(&result),
        _ => println!("{}", serde_json::to_string_pretty(&result).unwrap()),
    }
}

fn emit_human(hs: &HypothesisSet) {
    println!("Target: {} (delta: {:.2}%)", hs.target, hs.target_delta_fraction * 100.0);
    println!("Evidence type: {}", hs.evidence_type);
    println!("Note: {}", hs.analysis_note);
    println!();
    for h in &hs.hypotheses {
        println!(
            "  #{}: [{}] {} (confidence: {})",
            h.rank,
            match h.corroboration {
                Corroboration::Corroborated => "CORROBORATED",
                Corroboration::StructuralOnly => "STRUCTURAL",
            },
            h.explanation,
            h.confidence
        );
        println!("      Path: {}", h.path.join(" → "));
    }
}
