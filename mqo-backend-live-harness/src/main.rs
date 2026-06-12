//! mqo-live-harness — port-gated DAX/MDX E2E harness
//!
//! Usage:
//!   mqo-live-harness [--cases fixtures/default_cases.json] [--backends sql,dax,mdx]
//!   mqo-live-harness --corpus parity-corpus.json --build-id b-2026.1 [--out s.jsonl]
//!
//! Environment variables (live mode):
//!   ATSCALE_PGWIRE_HOST  — host for SQL/DAX (port 11120)
//!   ATSCALE_XMLA_URL     — XMLA endpoint for MDX (e.g. http://host:11111)

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;

use clap::Parser;

use mqo_backend_live_harness::{
    probe::{CapabilityProbe, EnvProbe, FakeProbe},
    runner, Backend,
    BackendStatus as HarnessBackendStatus,
    CorpusDocument, CorpusRunRecord, TestCase,
};
use mqo_cross_backend_parity as oracle;

// ---------------------------------------------------------------------------
// Real engine (no-op stub — replaced by real impl when deps land)
// ---------------------------------------------------------------------------

struct StubEngine;

impl runner::Engine for StubEngine {
    fn execute(&self, backend: Backend, case: &TestCase) -> Result<f64, String> {
        Err(format!(
            "StubEngine: real execution not yet wired for ({backend}, {})",
            case.name
        ))
    }
}

struct StubComparator;

impl mqo_backend_live_harness::comparator::ParityComparator for StubComparator {
    fn compare(
        &self,
        _case: &TestCase,
        backends: &[Backend],
    ) -> mqo_backend_live_harness::ParityOutcome {
        mqo_backend_live_harness::ParityOutcome::Skipped {
            reason: format!("stub comparator ({} backends)", backends.len()),
        }
    }
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "mqo-live-harness",
    about = "Port-gated DAX/MDX E2E harness — green-skips dead ports, flips pass when ports open"
)]
struct Cli {
    /// Path to JSON test-case file (bare array of TestCase objects).
    /// Default: fixtures/default_cases.json. Mutually exclusive with --corpus.
    #[arg(long)]
    cases: Option<PathBuf>,

    /// Path to parity-corpus.v1 document. Mutually exclusive with --cases.
    #[arg(long)]
    corpus: Option<PathBuf>,

    /// Comma-separated list of backends to probe: sql,dax,mdx
    #[arg(long, default_value = "sql,dax,mdx")]
    backends: String,

    /// Build id to stamp on every emitted record. Required when --out is supplied.
    #[arg(long)]
    build_id: Option<String>,

    /// Destination for the per-case ParityReport stream (JSONL).
    /// Use '-' or omit for stdout. Requires --build-id.
    #[arg(long)]
    out: Option<String>,
}

fn main() {
    let cli = Cli::parse();

    // FR3: --cases and --corpus are mutually exclusive.
    if cli.cases.is_some() && cli.corpus.is_some() {
        eprintln!(
            "error: --cases and --corpus are mutually exclusive; supply one source of cases"
        );
        std::process::exit(2);
    }

    // FR6: --out requires --build-id.
    if cli.out.is_some() && cli.build_id.is_none() {
        eprintln!(
            "error: --out requires --build-id; a build-stamped report requires a build id"
        );
        std::process::exit(2);
    }

    let backends = parse_backends(&cli.backends);
    if backends.is_empty() {
        eprintln!("error: no valid backends specified");
        std::process::exit(2);
    }

    if let Some(ref corpus_path) = cli.corpus {
        run_corpus_mode(corpus_path, &backends, &cli);
    } else {
        let cases_path = cli
            .cases
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("fixtures/default_cases.json"));
        run_cases_mode(cases_path, &backends);
    }
}

fn parse_backends(s: &str) -> Vec<Backend> {
    s.split(',')
        .filter_map(|tok| {
            let tok = tok.trim();
            if tok.is_empty() {
                None
            } else {
                match Backend::from_str(tok) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        eprintln!("warning: {e}");
                        None
                    }
                }
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// --cases mode (original behaviour, fully preserved)
// ---------------------------------------------------------------------------

fn run_cases_mode(cases_path: &std::path::Path, backends: &[Backend]) {
    let cases_json = match std::fs::read_to_string(cases_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read cases file {:?}: {e}", cases_path);
            std::process::exit(2);
        }
    };
    let cases: Vec<TestCase> = match serde_json::from_str(&cases_json) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: cannot parse cases file {:?}: {e}", cases_path);
            std::process::exit(2);
        }
    };

    let probe = EnvProbe;
    let engine = StubEngine;
    let comparator = StubComparator;
    let report = runner::run_harness(backends, &cases, &probe, &engine, &comparator);

    print!("{}", report.render());
    std::process::exit(if report.is_success() { 0 } else { 1 });
}

// ---------------------------------------------------------------------------
// --corpus mode: load corpus, run, emit per-case ParityReport stream (JSONL)
// ---------------------------------------------------------------------------

fn run_corpus_mode(corpus_path: &std::path::Path, backends: &[Backend], cli: &Cli) {
    // FR5: load and validate corpus.
    let corpus_json = match std::fs::read_to_string(corpus_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read corpus file {:?}: {e}", corpus_path);
            std::process::exit(2);
        }
    };
    let corpus_doc: CorpusDocument = match serde_json::from_str(&corpus_json) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: cannot parse corpus file {:?}: {e}", corpus_path);
            std::process::exit(2);
        }
    };
    if corpus_doc.version != "parity-corpus.v1" {
        eprintln!(
            "error: corpus file {:?} has version {:?}, expected \"parity-corpus.v1\"",
            corpus_path, corpus_doc.version
        );
        std::process::exit(2);
    }

    // FR2: map corpus cases → TestCase (case_id → name, mqo verbatim, expected_value = None).
    let cases: Vec<TestCase> = corpus_doc
        .cases
        .iter()
        .map(|c| TestCase {
            name: c.case_id.clone(),
            mqo: c.mqo.clone(),
            expected_value: None,
        })
        .collect();

    // Probe all backends once; reuse results for both run_harness and oracle report building.
    let probe_statuses: HashMap<Backend, HarnessBackendStatus> = backends
        .iter()
        .map(|&b| (b, EnvProbe.probe(b)))
        .collect();
    let fake_probe = FakeProbe::new(probe_statuses.clone());

    let engine = StubEngine;
    let comparator = StubComparator;
    // FR10: human-readable render → stderr in corpus mode.
    let report = runner::run_harness(backends, &cases, &fake_probe, &engine, &comparator);
    eprint!("{}", report.render());

    // Open output destination (FR8).
    let out_dest = cli.out.as_deref();
    let mut out_writer: Box<dyn Write> = match out_dest {
        None | Some("-") => Box::new(std::io::stdout()),
        Some(path) => match std::fs::File::create(path) {
            Ok(f) => Box::new(f),
            Err(e) => {
                eprintln!("error: cannot create output file {:?}: {e}", path);
                std::process::exit(2);
            }
        },
    };

    let build_id = cli.build_id.as_deref().unwrap_or("");
    let corpus_path_str = corpus_path.to_string_lossy().to_string();
    let backends_str: Vec<String> = backends.iter().map(|b| b.to_string()).collect();
    let oracle_comparator = oracle::comparator::StubComparator::default();

    // Emit per-case records in corpus case order (NFR4: stable ordering).
    for corpus_case in &corpus_doc.cases {
        // Map harness probe results to oracle BackendStatus.
        // Live probes yield Error (StubEngine not wired); non-live yield Skipped.
        let mut oracle_results: HashMap<String, oracle::BackendStatus> = HashMap::new();
        for (&backend, harness_status) in &probe_statuses {
            let oracle_status = match harness_status {
                HarnessBackendStatus::Live => oracle::BackendStatus::Error {
                    message: format!(
                        "StubEngine: real execution not yet wired for ({backend}, {})",
                        corpus_case.case_id
                    ),
                },
                HarnessBackendStatus::Rejected { reason } => oracle::BackendStatus::Skipped {
                    reason: format!("rejected: {reason}"),
                },
                HarnessBackendStatus::Unreachable { reason } => oracle::BackendStatus::Skipped {
                    reason: format!("unreachable: {reason}"),
                },
            };
            oracle_results.insert(backend.to_string(), oracle_status);
        }

        let pairs = oracle::run_parity(&backends_str, &oracle_results, &oracle_comparator);
        let parity_report = oracle::ParityReport::build(
            corpus_path_str.clone(),
            backends_str.clone(),
            oracle_results,
            pairs,
        );

        let record = CorpusRunRecord {
            case_id: corpus_case.case_id.clone(),
            build_id: build_id.to_string(),
            report: parity_report,
        };

        let line = match serde_json::to_string(&record) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "error: failed to serialize record for {}: {e}",
                    corpus_case.case_id
                );
                std::process::exit(1);
            }
        };
        if let Err(e) = writeln!(out_writer, "{line}") {
            eprintln!(
                "error: failed to write record for {}: {e}",
                corpus_case.case_id
            );
            std::process::exit(1);
        }
    }

    // Corpus runs always exit 0; failures are captured in per-case verdicts.
    std::process::exit(0);
}
