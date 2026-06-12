//! # mqo-vs-sql-bench
//!
//! Library crate re-exporting the benchmark modules for integration testing.

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
