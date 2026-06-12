//! `mcp-cross-cluster-diff` binary entry point.

use clap::Parser;
use mcp_cross_cluster_diff::{
    catalog::DescribeModel,
    diff::{diff_catalogs, exit_code, DiffConfig},
};
use std::fs;
use std::process;

/// Diff two AtScale cluster describe_model catalogs and classify divergences.
#[derive(Parser, Debug)]
#[command(name = "mcp-cross-cluster-diff", version, about)]
struct Args {
    /// Path to catalog A (describe_model JSON).
    #[arg(long)]
    catalog_a: String,

    /// Path to catalog B (describe_model JSON).
    #[arg(long)]
    catalog_b: String,

    /// Name of cluster A (for labeling).
    #[arg(long)]
    cluster_a: String,

    /// Name of cluster B (for labeling).
    #[arg(long)]
    cluster_b: String,

    /// Numeric tolerance as a fraction (default 0.001 = 0.1%).
    /// Reserved for future numeric stats comparison.
    #[arg(long, default_value = "0.001")]
    numeric_tolerance: f64,

    /// Output format: json (default) or human.
    #[arg(long, default_value = "json")]
    format: String,

    /// Optional output file path. If omitted, writes to stdout.
    #[arg(long)]
    output: Option<String>,
}

fn main() {
    let args = Args::parse();

    // Load catalog A
    let raw_a = fs::read_to_string(&args.catalog_a).unwrap_or_else(|e| {
        eprintln!("ERROR: cannot read --catalog-a '{}': {e}", args.catalog_a);
        process::exit(2);
    });
    let catalog_a = DescribeModel::from_json(&raw_a).unwrap_or_else(|e| {
        eprintln!("ERROR: cannot parse --catalog-a '{}': {e}", args.catalog_a);
        process::exit(2);
    });

    // Load catalog B
    let raw_b = fs::read_to_string(&args.catalog_b).unwrap_or_else(|e| {
        eprintln!("ERROR: cannot read --catalog-b '{}': {e}", args.catalog_b);
        process::exit(2);
    });
    let catalog_b = DescribeModel::from_json(&raw_b).unwrap_or_else(|e| {
        eprintln!("ERROR: cannot parse --catalog-b '{}': {e}", args.catalog_b);
        process::exit(2);
    });

    let config = DiffConfig {
        cluster_a: args.cluster_a.clone(),
        cluster_b: args.cluster_b.clone(),
        numeric_tolerance: args.numeric_tolerance,
    };

    let report = diff_catalogs(&catalog_a, &catalog_b, &config);
    let code = exit_code(&report.overall_verdict);

    // Format output
    let output_text = match args.format.as_str() {
        "human" => format_human(&report),
        _ => serde_json::to_string_pretty(&report).expect("report is always serializable"),
    };

    match args.output {
        Some(ref path) => {
            fs::write(path, &output_text).unwrap_or_else(|e| {
                eprintln!("ERROR: cannot write output to '{path}': {e}");
                process::exit(2);
            });
        }
        None => println!("{output_text}"),
    }

    process::exit(code);
}

fn format_human(report: &mcp_cross_cluster_diff::report::DiffReport) -> String {
    use mcp_cross_cluster_diff::report::OverallVerdict;

    let verdict_str = match report.overall_verdict {
        OverallVerdict::Agree => "AGREE",
        OverallVerdict::Diverge => "DIVERGE",
        OverallVerdict::CriticalDiverge => "CRITICAL_DIVERGE",
    };

    let mut lines = vec![
        "=== mcp-cross-cluster-diff ===".to_string(),
        format!("Cluster A : {}", report.clusters.a),
        format!("Cluster B : {}", report.clusters.b),
        format!("Verdict   : {verdict_str}"),
        String::new(),
        "Summary:".to_string(),
        format!("  agree            : {}", report.summary.agree),
        format!("  diverge          : {}", report.summary.diverge),
        format!("  critical_diverge : {}", report.summary.critical_diverge),
        format!("  only_in_a        : {}", report.summary.only_in_a),
        format!("  only_in_b        : {}", report.summary.only_in_b),
    ];

    if !report.only_in_a.is_empty() {
        lines.push(String::new());
        lines.push("Only in A:".to_string());
        for e in &report.only_in_a {
            lines.push(format!("  [{}] {}", e.entity_type, e.unique_name));
        }
    }

    if !report.only_in_b.is_empty() {
        lines.push(String::new());
        lines.push("Only in B:".to_string());
        for e in &report.only_in_b {
            lines.push(format!("  [{}] {}", e.entity_type, e.unique_name));
        }
    }

    if !report.differences.is_empty() {
        lines.push(String::new());
        lines.push("Differences:".to_string());
        for d in &report.differences {
            if d.field_diffs.is_empty() {
                continue;
            }
            lines.push(format!(
                "  [{}] {} => {:?}",
                d.entity_type, d.unique_name, d.verdict
            ));
            for fd in &d.field_diffs {
                let crit = if fd.critical { " [CRITICAL]" } else { "" };
                lines.push(format!(
                    "    .{}: A={:?} B={:?}{}",
                    fd.field,
                    fd.cluster_a.as_deref().unwrap_or("(none)"),
                    fd.cluster_b.as_deref().unwrap_or("(none)"),
                    crit
                ));
            }
        }
    }

    lines.join("\n")
}
