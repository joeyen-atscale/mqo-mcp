//! # dh-spec
//!
//! Dataset-Handle protocol types — the shared vocabulary every component of the
//! `dh-*` MCP fleet compiles against.
//!
//! This crate provides **only the wire shapes and their JSON Schema** — no
//! execution logic.  The core contract: an LLM receives a
//! [`DatasetSummary`] + [`DatasetHandle`] + [`Vec<Capability>`], never a raw
//! dataset.
//!
//! ## Quick start
//!
//! ```rust
//! use dh_spec::{DatasetHandle, DatasetSummary, Capability, INLINE_THRESHOLD};
//!
//! // Build a handle for a freshly-computed result set.
//! let handle = DatasetHandle {
//!     id: "hdl_abc123".to_string(),
//!     created_at: 1_717_000_000,
//!     ttl_secs: 3600,
//!     derived_from: None,
//! };
//!
//! // Datasets with ≤ INLINE_THRESHOLD rows may be returned inline.
//! assert!(INLINE_THRESHOLD <= 8);
//!
//! // A result set larger than INLINE_THRESHOLD always needs a handle.
//! let row_count: u64 = 1_000;
//! let needs_handle = row_count > INLINE_THRESHOLD as u64;
//! assert!(needs_handle);
//!
//! // Available capabilities are statically known.
//! let caps = vec![Capability::Aggregate, Capability::Filter];
//! assert!(!caps.is_empty());
//! ```

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

// ── Constants ──────────────────────────────────────────────────────────────

/// Scalars or result sets of this many rows or fewer **may** be returned
/// inline (without minting a handle).  Above this threshold a handle is
/// **mandatory** — the LLM must not receive the raw dataset.
///
/// Engineering note: this is the confirmable knob.  Changing it affects
/// every fleet member that inlines results, so treat changes as a
/// breaking-schema update and bump the crate's minor version.
pub const INLINE_THRESHOLD: usize = 8;

/// Default maximum number of sample rows stored in a [`DatasetSummary`].
///
/// This is the default; individual summaries may use a smaller cap.
pub const DEFAULT_SAMPLE_CAP: usize = 20;

// ── DatasetHandle ──────────────────────────────────────────────────────────

/// An opaque, server-minted reference to a result set held in server memory.
///
/// The LLM holds a `DatasetHandle` and issues [`OpRequest`]s against it;
/// it never receives the raw rows.
///
/// `DatasetHandle` is a newtype over a `String` id with provenance fields.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Opaque server-minted reference to a stored result set")]
pub struct DatasetHandle {
    /// Server-minted opaque identifier (e.g. a UUID or `hdl_<base62>`).
    pub id: String,

    /// Unix timestamp (seconds) when this handle was created.
    pub created_at: i64,

    /// How long the server will keep this handle alive, in seconds.
    pub ttl_secs: u64,

    /// If this handle was produced by an operation on another handle, that
    /// parent handle's id is recorded here for lineage tracing.
    pub derived_from: Option<Box<DatasetHandle>>,
}

// ── ColumnSchema ───────────────────────────────────────────────────────────

/// The data type of a column value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub enum DType {
    /// 64-bit signed integer.
    Int,
    /// 64-bit IEEE 754 float.
    Float,
    /// Arbitrary-precision decimal (wire type: string).
    Decimal,
    /// UTF-8 string.
    Str,
    /// Boolean.
    Bool,
    /// Calendar date (ISO 8601, no time component).
    Date,
    /// Time of day or timestamp (ISO 8601).
    Time,
}

/// The semantic role of a column in the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub enum ColumnRole {
    /// An aggregatable numeric metric.
    Measure,
    /// A grouping / slicing attribute.
    Dimension,
    /// A post-query computed column (e.g. time-intelligence, expression).
    Derived,
}

/// Metadata describing one column of a result set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Metadata for one column of a result set")]
pub struct ColumnSchema {
    /// Display name as it appears in the result set.
    pub name: String,

    /// Fully-qualified unique name in the semantic model.
    pub unique_name: String,

    /// Wire data type.
    pub dtype: DType,

    /// True when this column may contain SQL NULL / JSON null values.
    pub nullable: bool,

    /// Semantic role of this column.
    pub role: ColumnRole,
}

// ── ColStats ───────────────────────────────────────────────────────────────

/// Per-column statistics included in a [`DatasetSummary`].
///
/// Numeric and categorical columns expose different stat fields; absent
/// fields are serialized as JSON `null`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Per-column statistics (numeric and categorical)")]
pub struct ColStats {
    /// Minimum value (numerics only).
    pub min: Option<f64>,
    /// Maximum value (numerics only).
    pub max: Option<f64>,
    /// Sum (numerics only).
    pub sum: Option<f64>,
    /// Mean (numerics only).
    pub mean: Option<f64>,
    /// Count of distinct values.
    pub distinct: Option<u64>,
    /// Top-k most-frequent values (categoricals; absent for pure numerics).
    pub top_k: Option<Vec<Value>>,
}

/// A single row of result data: an ordered map from column name to JSON value.
pub type Row = HashMap<String, Value>;

// ── DatasetSummary ─────────────────────────────────────────────────────────

/// The bounded view of a result set that the LLM receives.
///
/// A `DatasetSummary` is **never** the full dataset: `sample` is capped at
/// `sample_cap` rows regardless of how large the underlying result set is.
///
/// # Invariant
///
/// `sample.len() <= sample_cap` — enforced by [`DatasetSummary::new`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Bounded summary of a result set — never the full dataset")]
pub struct DatasetSummary {
    /// Total number of rows in the full (server-side) result set.
    pub row_count: u64,

    /// Schema of all columns.
    pub columns: Vec<ColumnSchema>,

    /// Head/tail sample of rows, bounded by `sample_cap`.
    pub sample: Vec<Row>,

    /// Maximum number of rows that may appear in `sample`.
    pub sample_cap: usize,

    /// Per-column statistics keyed by `ColumnSchema::unique_name`.
    pub stats: HashMap<String, ColStats>,

    /// Free-form notes (warnings, truncation notices, etc.).
    pub notes: Vec<String>,
}

impl DatasetSummary {
    /// Construct a `DatasetSummary`, capping `sample` at `sample_cap`.
    ///
    /// If `rows` is longer than `sample_cap` the head rows up to `sample_cap`
    /// are kept and a truncation notice is appended to `notes`.
    #[must_use]
    pub fn new(
        row_count: u64,
        columns: Vec<ColumnSchema>,
        mut rows: Vec<Row>,
        sample_cap: usize,
        stats: HashMap<String, ColStats>,
        mut notes: Vec<String>,
    ) -> Self {
        if rows.len() > sample_cap {
            rows.truncate(sample_cap);
            notes.push(format!(
                "sample truncated to {sample_cap} rows (full result: {row_count} rows)"
            ));
        }
        Self {
            row_count,
            columns,
            sample: rows,
            sample_cap,
            stats,
            notes,
        }
    }
}

// ── Capability ─────────────────────────────────────────────────────────────

/// An operation available on a [`DatasetHandle`].
///
/// Eleven variants: nine data-transformation ops plus two visualization ops
/// (`Chart`, `BiAsset`) added in v0.2.0 of the dataset-handle fleet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
pub enum Capability {
    /// Group-by aggregation (SUM, COUNT, AVG, …).
    Aggregate,
    /// Row filtering by predicate.
    Filter,
    /// Sort by one or more columns.
    Sort,
    /// Return only the top-N rows by a metric.
    TopN,
    /// Pivot rows to columns (cross-tab).
    Pivot,
    /// Compare two or more sub-groups or time periods.
    Compare,
    /// Drill into a finer granularity within a hierarchy.
    Drill,
    /// Produce a natural-language description of the result.
    Describe,
    /// Export the result to a downstream format (CSV, Parquet, …).
    Export,
    /// Emit a Vega-Lite v5 chart spec from a handle (no rows returned).
    Chart,
    /// Emit a full BI asset bundle (title, description, spec, caveats) from a handle.
    BiAsset,
}

/// All eleven [`Capability`] variants, in declaration order.
///
/// Useful for iterating or asserting the complete capability set.
pub const ALL_CAPABILITIES: [Capability; 11] = [
    Capability::Aggregate,
    Capability::Filter,
    Capability::Sort,
    Capability::TopN,
    Capability::Pivot,
    Capability::Compare,
    Capability::Drill,
    Capability::Describe,
    Capability::Export,
    Capability::Chart,
    Capability::BiAsset,
];

// ── OpRequest / OpResult ───────────────────────────────────────────────────

/// A request to apply an [`Capability`] operation to an existing handle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Request to apply an operation to a stored dataset handle")]
pub struct OpRequest {
    /// The handle to operate on.
    pub handle: DatasetHandle,

    /// The operation to apply.
    pub op: Capability,

    /// Operation-specific parameters (validated by the server against the
    /// capability's param schema).
    pub params: Value,
}

/// The result of an [`OpRequest`]: a **new** handle + summary.
///
/// `handle` is always a freshly-minted server-side id — distinct from the
/// request's input handle — because operations are non-destructive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Result of an operation: a new handle and its summary")]
pub struct OpResult {
    /// The new handle for the derived result set.
    pub handle: DatasetHandle,

    /// Bounded summary of the derived result set.
    pub summary: DatasetSummary,
}

// ── Lineage ────────────────────────────────────────────────────────────────

/// Provenance record for a derived handle.
///
/// Every time an [`OpRequest`] produces an [`OpResult`], a `Lineage` record
/// MAY be persisted by the server for audit or replay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Provenance record linking a derived handle to its inputs")]
pub struct Lineage {
    /// The derived handle produced by this operation.
    pub handle: DatasetHandle,

    /// The operation that was applied.
    pub op: Capability,

    /// The operation parameters as supplied by the caller.
    pub params: Value,

    /// All input handles consumed by this operation.
    ///
    /// Most ops have a single parent; `Compare` and multi-join ops may have
    /// two or more.
    pub parents: Vec<DatasetHandle>,
}

// ── Schema emission ────────────────────────────────────────────────────────

/// Emit the JSON Schema for [`DatasetSummary`] as a pretty-printed string.
///
/// # Panics
///
/// Panics only if `serde_json` fails to serialize the schema, which cannot
/// happen in practice for a schema derived from a concrete struct.
#[must_use]
pub fn emit_summary_schema() -> String {
    let schema = schemars::schema_for!(DatasetSummary);
    serde_json::to_string_pretty(&schema).expect("schemars schema is always serializable")
}

/// Emit the JSON Schema for [`OpResult`] as a pretty-printed string.
///
/// # Panics
///
/// Panics only if `serde_json` fails to serialize the schema.
#[must_use]
pub fn emit_op_result_schema() -> String {
    let schema = schemars::schema_for!(OpResult);
    serde_json::to_string_pretty(&schema).expect("schemars schema is always serializable")
}

// ── Unit tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod unit_tests {
    use super::*;

    fn make_handle(id: &str) -> DatasetHandle {
        DatasetHandle {
            id: id.to_string(),
            created_at: 1_717_000_000,
            ttl_secs: 3600,
            derived_from: None,
        }
    }

    fn make_column(name: &str, dtype: DType, role: ColumnRole) -> ColumnSchema {
        ColumnSchema {
            name: name.to_string(),
            unique_name: format!("model.{name}"),
            dtype,
            nullable: false,
            role,
        }
    }

    fn make_summary_with_rows(rows: Vec<Row>, sample_cap: usize) -> DatasetSummary {
        let row_count = rows.len() as u64;
        DatasetSummary::new(
            row_count,
            vec![make_column("revenue", DType::Float, ColumnRole::Measure)],
            rows,
            sample_cap,
            HashMap::new(),
            vec![],
        )
    }

    #[test]
    fn inline_threshold_is_eight_or_fewer() {
        assert!(INLINE_THRESHOLD <= 8);
    }

    #[test]
    fn handle_round_trips_json() {
        let h = make_handle("hdl_test");
        let json = serde_json::to_string(&h).expect("serialize");
        let h2: DatasetHandle = serde_json::from_str(&json).expect("deserialize"); // allowlist: test-only; from_str on self-serialized JSON cannot fail
        assert_eq!(h, h2);
    }

    #[test]
    fn summary_truncates_at_sample_cap() {
        let rows: Vec<Row> = (0..100)
            .map(|i| {
                let mut r = HashMap::new();
                r.insert("revenue".to_string(), Value::from(i));
                r
            })
            .collect();
        let summary = make_summary_with_rows(rows, 20);
        assert_eq!(summary.sample.len(), 20);
        assert!(!summary.notes.is_empty(), "truncation note expected");
    }

    #[test]
    fn emit_summary_schema_is_valid_json() {
        let s = emit_summary_schema();
        let v: Value = serde_json::from_str(&s).expect("valid JSON"); // allowlist: test-only; schemars emit is always valid JSON
        assert!(v.is_object());
    }

    #[test]
    fn emit_op_result_schema_is_valid_json() {
        let s = emit_op_result_schema();
        let v: Value = serde_json::from_str(&s).expect("valid JSON"); // allowlist: test-only; schemars emit is always valid JSON
        assert!(v.is_object());
    }
}
