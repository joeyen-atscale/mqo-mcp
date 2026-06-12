use clap::{Parser, Subcommand};
use mqo_bench_history::ingest::{has_regression, ingest, print_ingest_result};
use mqo_bench_history::report::run_report;
use std::path::PathBuf;
use std::process;

#[derive(Parser)]
#[command(name = "mqo-bench-history", about = "Longitudinal regression tracker for mqo-bench runs")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Ingest {
        bench_output: PathBuf,
        #[arg(long, default_value = "~/.local/share/mqo-bench-history/runs.jsonl")]
        history_file: String,
        #[arg(long, default_value_t = 5)]
        baseline_window: usize,
        #[arg(long, default_value_t = 5.0)]
        regress_threshold: f64,
    },
    Report {
        #[arg(long, default_value_t = 10)]
        last: usize,
        #[arg(long)]
        csv: bool,
        #[arg(long, default_value = "~/.local/share/mqo-bench-history/runs.jsonl")]
        history_file: String,
    },
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(format!("{home}/{rest}"));
        }
    }
    PathBuf::from(path)
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Ingest {
            bench_output,
            history_file,
            baseline_window,
            regress_threshold,
        } => {
            let history_path = expand_tilde(&history_file);
            match ingest(&bench_output, &history_path, baseline_window, regress_threshold) {
                Ok(result) => {
                    print_ingest_result(&result);
                    if has_regression(&result) {
                        process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    process::exit(2);
                }
            }
        }
        Commands::Report {
            last,
            csv,
            history_file,
        } => {
            let history_path = expand_tilde(&history_file);
            match run_report(&history_path, last, csv) {
                Ok(()) => {}
                Err(e) => {
                    eprintln!("Error: {e}");
                    process::exit(2);
                }
            }
        }
    }
}
