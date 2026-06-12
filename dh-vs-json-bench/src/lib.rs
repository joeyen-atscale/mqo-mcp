//! # dh-vs-json-bench
//!
//! Library crate re-exporting the benchmark modules for integration testing.
//!
//! Measures the LLM-as-calculator failure: arm A (raw-JSON) hands the model
//! full result rows and asks it to compute the answer itself; arm B (handle)
//! gives the model a summary+handle and lets it orchestrate `dataset_*` tools
//! so the server computes every reported value.

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::similar_names,
    clippy::cast_precision_loss,
    clippy::doc_markdown,
    clippy::must_use_candidate
)]

pub mod grader;
pub mod report;
pub mod runner;
pub mod types;
