//! `mcp-cross-cluster-diff` — diff two AtScale cluster describe_model catalogs.
//!
//! Loads two describe_model JSONs, compares measures and dimensions by `unique_name`,
//! classifies each entity as agree/diverge/critical_diverge/only_in_a/only_in_b,
//! and returns a structured [`DiffReport`].

pub mod catalog;
pub mod diff;
pub mod report;
