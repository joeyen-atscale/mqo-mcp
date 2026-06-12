//! `mcp-cluster-health-monitor` binary entry point.
//!
//! Exit codes:
//! - 0: overall healthy
//! - 1: degraded or critical
//! - 2: registry parse error

use clap::Parser;
use mcp_cluster_health_monitor::{
    probe::probe_clusters,
    report::{HealthReport, OverallStatus},
};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "mcp-cluster-health-monitor")]
#[command(about = "Health canary for registered AtScale clusters")]
struct Args {
    /// Path to cluster registry (TOML or JSON)
    #[arg(long, default_value = "registry.toml")]
    registry: PathBuf,

    /// Per-cluster probe timeout in seconds (legacy; prefer --timeout-ms)
    #[arg(long = "timeout-secs", default_value = "5")]
    timeout_secs: u64,

    /// Per-cluster probe timeout in milliseconds (overrides --timeout-secs if provided)
    #[arg(long = "timeout-ms")]
    timeout_ms: Option<u64>,

    /// Output format
    #[arg(long, default_value = "json")]
    format: OutputFormat,

    /// Only probe the named cluster (may be repeated)
    #[arg(long = "cluster")]
    clusters: Vec<String>,
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum OutputFormat {
    Json,
    Human,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    // Determine timeout_ms: explicit --timeout-ms wins, else convert --timeout-secs
    let timeout_ms = args.timeout_ms.unwrap_or(args.timeout_secs * 1000);

    // Read registry file
    let registry_str = match std::fs::read_to_string(&args.registry) {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "registry read error: {}: {}",
                args.registry.display(),
                e
            );
            std::process::exit(2);
        }
    };

    // Parse registry (TOML or JSON based on extension)
    let ext = args
        .registry
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("toml");

    let registry = if ext == "json" {
        mcp_cluster_registry::ClusterRegistry::from_json(&registry_str)
    } else {
        mcp_cluster_registry::ClusterRegistry::from_toml(&registry_str)
    };

    let registry = match registry {
        Ok(r) => r,
        Err(e) => {
            eprintln!("registry parse error: {e}");
            std::process::exit(2);
        }
    };

    // Filter clusters if --cluster was specified
    let clusters_to_probe: Vec<&mcp_cluster_registry::ClusterEntry> = if args.clusters.is_empty() {
        registry.clusters.iter().collect()
    } else {
        registry
            .clusters
            .iter()
            .filter(|c| args.clusters.iter().any(|n| n == &c.name))
            .collect()
    };

    if clusters_to_probe.is_empty() {
        eprintln!("no clusters to probe");
        std::process::exit(2);
    }

    // Run concurrent probes
    let health = probe_clusters(&clusters_to_probe, timeout_ms).await;
    let report = HealthReport::from_health(&health);

    // Output
    match args.format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
        }
        OutputFormat::Human => {
            println!("overall: {}", report.overall);
            for c in &report.clusters {
                let latency = c
                    .latency_ms
                    .map(|l| format!(" {l}ms"))
                    .unwrap_or_default();
                let error = c
                    .error
                    .as_deref()
                    .map(|e| format!(" error={e}"))
                    .unwrap_or_default();
                println!("  {} [{}]{}{}", c.name, c.status, latency, error);
            }
        }
    }

    // Exit code
    let exit_code = match report.overall {
        OverallStatus::Healthy => 0,
        OverallStatus::Degraded => 1,
        OverallStatus::Critical => 1,
    };
    std::process::exit(exit_code);
}
