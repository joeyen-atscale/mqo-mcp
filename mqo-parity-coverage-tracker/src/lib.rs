//! # mqo-parity-coverage-tracker
//!
//! Records parity verdicts (verified | mismatch | never-tested) per
//! (measure, backend-pair) against server builds in an append-only JSONL
//! history store.
//!
//! ## Subcommand contracts
//!
//! * **record** — ingest a `ParityReport` or live-harness per-case results,
//!   stamp with `--build-id`, append one entry to the JSONL store.
//! * **coverage** — read history + corpus, emit per-(measure, pair) status
//!   and an overall coverage % for a given build id.
//! * **delta** — diff two recorded builds; report newly-verified,
//!   newly-broken, and newly-tested measures.
//!
//! ## Default store path
//!
//! `~/.local/share/mqo-parity/history.jsonl`
//!
//! The path is overridable at every call site via the `store_path` / `--history`
//! argument; no global mutable state is used.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![deny(clippy::all)]

pub mod corpus;
pub mod history;
pub mod store;
pub mod summary;

pub use corpus::{Corpus, CorpusCase};
pub use history::{HistoryEntry, PairRecord, ParsedVerdict};
pub use store::{load_history, record_run};
pub use summary::{coverage_summary, delta_summary, CoverageSummary, DeltaSummary};

use std::path::{Path, PathBuf};

/// Returns the default history store path: `~/.local/share/mqo-parity/history.jsonl`.
#[must_use]
pub fn default_store_path() -> PathBuf {
    let base = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    Path::new(&base)
        .join(".local")
        .join("share")
        .join("mqo-parity")
        .join("history.jsonl")
}
