//! mqo-backend-live-harness library
//!
//! Types, traits, and logic for the port-gated DAX/MDX E2E harness.
//! Real backend impls (probe, compiler, executor) are injected via traits so
//! tests can run entirely offline with fakes.

pub mod comparator;
pub mod probe;
pub mod runner;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Backend kinds
// ---------------------------------------------------------------------------

/// The two query-path backends the harness understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Sql,
    Dax,
    Mdx,
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Backend::Sql => write!(f, "sql"),
            Backend::Dax => write!(f, "dax"),
            Backend::Mdx => write!(f, "mdx"),
        }
    }
}

impl std::str::FromStr for Backend {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "sql" => Ok(Backend::Sql),
            "dax" => Ok(Backend::Dax),
            "mdx" => Ok(Backend::Mdx),
            other => Err(format!("unknown backend: {other}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Capability probe result
// ---------------------------------------------------------------------------

/// Whether a backend is reachable and ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendStatus {
    /// Port open, protocol handshake succeeded.
    Live,
    /// Port open but protocol refused the request (e.g. PGWire rejected EVALUATE).
    Rejected { reason: String },
    /// Port unreachable / timed out.
    Unreachable { reason: String },
}

impl BackendStatus {
    pub fn is_live(&self) -> bool {
        matches!(self, BackendStatus::Live)
    }

    pub fn skip_reason(&self) -> Option<String> {
        match self {
            BackendStatus::Live => None,
            BackendStatus::Rejected { reason } => Some(format!("rejected: {reason}")),
            BackendStatus::Unreachable { reason } => Some(format!("unreachable: {reason}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Test case definition (loaded from JSON)
// ---------------------------------------------------------------------------

/// A single MQO test case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestCase {
    /// Human-readable name, used in output.
    pub name: String,
    /// The MQO payload (kept as raw JSON so the harness stays schema-agnostic).
    pub mqo: serde_json::Value,
    /// Expected scalar result for assertion. When `None` the value-assertion lane is
    /// skipped and the case relies solely on the cross-backend parity check.
    #[serde(default)]
    pub expected_value: Option<f64>,
}

// ---------------------------------------------------------------------------
// parity-corpus.v1 input types (FR2, FR5)
// ---------------------------------------------------------------------------

/// Top-level envelope of a `parity-corpus.v1` document.
#[derive(Debug, Deserialize)]
pub struct CorpusDocument {
    pub version: String,
    pub catalog: String,
    pub cases: Vec<CorpusCase>,
}

/// One case entry inside a `parity-corpus.v1` document.
/// Unknown fields are silently ignored so the harness tolerates future corpus extensions.
#[derive(Debug, Deserialize)]
pub struct CorpusCase {
    pub case_id: String,
    pub mqo: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Per-case emitted record (FR7, FR8)
// ---------------------------------------------------------------------------

/// One JSONL record emitted by the corpus run path.
/// Carries `case_id` and `build_id` alongside all `ParityReport` fields (flattened).
#[derive(Debug, Serialize)]
pub struct CorpusRunRecord {
    pub case_id: String,
    pub build_id: String,
    #[serde(flatten)]
    pub report: mqo_cross_backend_parity::ParityReport,
}

// ---------------------------------------------------------------------------
// Per-check outcome
// ---------------------------------------------------------------------------

/// Outcome for one (backend, case) pair.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckOutcome {
    Pass,
    Skip { reason: String },
    Fail { reason: String },
}

impl CheckOutcome {
    pub fn icon(&self) -> &'static str {
        match self {
            CheckOutcome::Pass => "✅",
            CheckOutcome::Skip { .. } => "⏭️",
            CheckOutcome::Fail { .. } => "❌",
        }
    }
}

/// Full record for one executed check.
#[derive(Debug, Clone)]
pub struct CaseResult {
    pub backend: Backend,
    pub case_name: String,
    pub outcome: CheckOutcome,
}

// ---------------------------------------------------------------------------
// Parity check result
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum ParityOutcome {
    /// All live backends agreed.
    Agreed,
    /// Two backends returned different values.
    Diverged {
        backend_a: Backend,
        value_a: f64,
        backend_b: Backend,
        value_b: f64,
        case_name: String,
    },
    /// Not enough live backends to compare.
    Skipped { reason: String },
}

// ---------------------------------------------------------------------------
// Harness report
// ---------------------------------------------------------------------------

/// Final summary produced by [`runner::run_harness`].
#[derive(Debug)]
pub struct HarnessReport {
    pub results: Vec<CaseResult>,
    pub parity: Vec<ParityOutcome>,
}

impl HarnessReport {
    pub fn passed(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.outcome == CheckOutcome::Pass)
            .count()
    }

    pub fn skipped(&self) -> usize {
        self.results
            .iter()
            .filter(|r| matches!(r.outcome, CheckOutcome::Skip { .. }))
            .count()
    }

    pub fn failed(&self) -> usize {
        self.results
            .iter()
            .filter(|r| matches!(r.outcome, CheckOutcome::Fail { .. }))
            .count()
    }

    /// True iff all non-skipped checks passed and no parity divergences.
    pub fn is_success(&self) -> bool {
        self.failed() == 0
            && self
                .parity
                .iter()
                .all(|p| !matches!(p, ParityOutcome::Diverged { .. }))
    }

    /// Render the checklist lines + summary line to a String.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for r in &self.results {
            let detail = match &r.outcome {
                CheckOutcome::Pass => String::new(),
                CheckOutcome::Skip { reason } => format!(" ({reason})"),
                CheckOutcome::Fail { reason } => format!(" — {reason}"),
            };
            out.push_str(&format!(
                "{} [{backend}] {name}{detail}\n",
                r.outcome.icon(),
                backend = r.backend,
                name = r.case_name,
            ));
        }
        for p in &self.parity {
            match p {
                ParityOutcome::Agreed => {
                    out.push_str("✅ [parity] all live backends agree\n");
                }
                ParityOutcome::Diverged {
                    backend_a,
                    value_a,
                    backend_b,
                    value_b,
                    case_name,
                } => {
                    out.push_str(&format!(
                        "❌ [parity] {case_name}: {backend_a}={value_a} vs {backend_b}={value_b}\n"
                    ));
                }
                ParityOutcome::Skipped { reason } => {
                    out.push_str(&format!("⏭️ [parity] skipped ({reason})\n"));
                }
            }
        }
        out.push_str(&format!(
            "{}/{} passed, {} skipped, {} failed\n",
            self.passed(),
            self.passed() + self.failed(),
            self.skipped(),
            self.failed(),
        ));
        out
    }
}
