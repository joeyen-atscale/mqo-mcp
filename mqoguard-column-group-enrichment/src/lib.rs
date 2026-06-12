//! `mqoguard-column-group-enrichment` — enriches `AtScale` catalog snapshots with
//! `column_group` tags identifying which fact table(s) each measure or dimension
//! level belongs to.
//!
//! # Overview
//!
//! The `AtScale` `describe_model` catalog payload exposes per-column metadata
//! (`unique_name`, `label`, `kind`, `is_calc`, `hierarchy`, `level`) but does
//! **not** indicate which fact table a column is bound to. Without this tag,
//! downstream guardrail crates cannot determine whether a measure×dimension pair
//! is queryable (e.g. "inventory quantity × promotions" is invalid because
//! inventory and promotions belong to different facts).
//!
//! This crate provides a deterministic enrichment pass:
//!
//! ```rust,no_run
//! use mqoguard_column_group_enrichment::{enrich, CatalogSnapshot, FactBindings};
//!
//! let catalog: CatalogSnapshot = serde_json::from_str("{}").unwrap_or_default();
//! let bindings = FactBindings::tpcds_defaults();
//! let enriched = enrich(&catalog, &bindings);
//! assert!(enriched.coverage.coverage_pct >= 0.0);
//! ```
//!
//! # Schema
//!
//! Output is `enriched-catalog.v1`. Every consumer that ignores `column_group`
//! is unaffected (FR4 — additive only).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod bindings;
mod catalog;
mod enrich;

pub use bindings::{FactBindings, FactBindingsError};
pub use catalog::{CatalogColumn, CatalogSnapshot};
pub use enrich::{enrich, CoverageReport, EnrichedCatalog, EnrichedColumn};
