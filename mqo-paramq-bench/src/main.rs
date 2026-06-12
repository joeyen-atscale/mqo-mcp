use clap::{Parser, ValueEnum};
use mqo_param_validator::CatalogSnapshot;
use mqo_paramq_bench::{render_markdown, run_bench, CandidateFile, CorpusTask};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "mqo-paramq-bench",
    about = "Offline pass@k bench: free-form vs structured MQO"
)]
struct Cli {
    #[arg(long)]
    corpus: PathBuf,

    #[arg(long)]
    freeform_candidates: PathBuf,

    #[arg(long)]
    structured_candidates: PathBuf,

    #[arg(long)]
    catalog: PathBuf,

    #[arg(long, default_value = "1")]
    k: usize,

    #[arg(long, default_value = "markdown")]
    format: OutputFormat,
}

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    Json,
    Markdown,
}

fn main() {
    let cli = Cli::parse();

    let corpus: Vec<CorpusTask> = {
        let raw = std::fs::read_to_string(&cli.corpus).expect("reading corpus");
        serde_json::from_str(&raw).expect("parsing corpus")
    };

    let freeform: CandidateFile = {
        let raw =
            std::fs::read_to_string(&cli.freeform_candidates).expect("reading freeform candidates");
        CandidateFile(serde_json::from_str(&raw).expect("parsing freeform"))
    };

    let structured_candidates: CandidateFile = {
        let raw = std::fs::read_to_string(&cli.structured_candidates)
            .expect("reading structured candidates");
        CandidateFile(serde_json::from_str(&raw).expect("parsing structured"))
    };

    let catalog: CatalogSnapshot = {
        let raw = std::fs::read_to_string(&cli.catalog).expect("reading catalog");
        serde_json::from_str(&raw).expect("parsing catalog")
    };

    let report = run_bench(&corpus, &freeform, &structured_candidates, &catalog, cli.k);

    match cli.format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("serializing report")
            );
        }
        OutputFormat::Markdown => {
            print!("{}", render_markdown(&report));
        }
    }
}
