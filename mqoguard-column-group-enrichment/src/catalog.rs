//! Catalog snapshot types — deserialized from the `AtScale` `describe_model` payload.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A raw column as returned by `AtScale` `describe_model`.
///
/// The shape matches the live fixture at
/// `~/Documents/projects/mqo-mcp-server/fixtures/tpcds_catalog.json`.
/// All fields are `Option` to tolerate partial or evolving payloads (AC6 — never panic).
// serde_json::Value does not implement Eq, so we cannot derive Eq here.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CatalogColumn {
    /// Fully-qualified unique name, e.g. `tpcds_benchmark_model.inventory_quantity_on_hand`.
    pub unique_name: String,
    /// Human-readable label.
    pub label: Option<String>,
    /// Column kind: `"measure"` or `"level"`.
    pub kind: Option<String>,
    /// Hierarchy name (for levels), e.g. `"inventory_date_dimensions"`.
    pub hierarchy: Option<String>,
    /// Level name (for levels), e.g. `"Inventory Date"`.
    pub level: Option<String>,
    /// Whether this is a calculated measure/dimension.
    pub is_calc: Option<bool>,
    /// Pass-through of any additional fields from the payload (forward-compat).
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// The full catalog snapshot returned by `describe_model`.
///
/// Deserializes from the top-level JSON object; `columns` is the list of
/// catalog entities this library enriches.
// serde_json::Value does not implement Eq, so we cannot derive Eq here.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CatalogSnapshot {
    /// Optional catalog name (metadata from the payload).
    pub catalog: Option<String>,
    /// Optional schema name.
    pub schema: Option<String>,
    /// The columns to enrich.
    #[serde(default)]
    pub columns: Vec<CatalogColumn>,
    /// Pass-through of any additional top-level fields.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}
