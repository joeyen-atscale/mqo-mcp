//! # dh-mcp-server
//!
//! The **keystone** of the dataset-handle MCP fleet.
//!
//! `query_multidimensional` runs the existing MQO `bind→route→compile→execute`
//! pipeline (reusing the [`mqo_mcp_server`] library as its data source) but,
//! instead of returning rows, stores the result in [`dh_store`] and returns
//! `{ summary, handle, capabilities }`.  A `dataset_*` tool family then lets an
//! LLM work the data **in place** — every operation derives a new handle and
//! returns only a bounded summary, so the rows never enter the context window.
//!
//! Read-only and deterministic by construction:
//!
//! * the only query input is a selection-only MQO (raw SQL / non-MQO input is
//!   rejected with a structured error — there is no SQL passthrough);
//! * no tool returns more than `sample_cap` rows **except** the explicit
//!   [`dataset_export`](mcp) path, which produces an audited
//!   [`dh_export::ExportReceipt`];
//! * every advertised tool carries `readOnlyHint: true`;
//! * the store's TTL + LRU eviction is wired into the request loop.

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

pub mod chart_tools;
pub mod convert;
pub mod mcp;

pub use mcp::{tool_descriptors, Server, CHART_TOOLS, DATASET_TOOLS, DEFAULT_TTL_SECS, PROTOCOL_VERSION};

// Re-export the fleet path resolver so binaries / tests can build a Server.
pub use mqo_mcp_server::pipeline::ToolPaths;

use dh_spec::DatasetSummary;
use dh_store::Dataset;
use dh_summary::{summarize, SummaryCfg};

/// Default total-bytes cap for the in-memory store (256 MiB).
pub const DEFAULT_MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;

/// Default summary sample cap advertised by the server.
pub const DEFAULT_SAMPLE_CAP: usize = dh_spec::DEFAULT_SAMPLE_CAP;

/// Summarize a dataset with an explicit sample cap, leaving the other
/// [`SummaryCfg`] knobs at their defaults.
#[must_use]
pub fn summarize_with_cap(dataset: &Dataset, sample_cap: usize) -> DatasetSummary {
    let cfg = SummaryCfg {
        sample_cap,
        ..SummaryCfg::default()
    };
    summarize(dataset, &cfg)
}
