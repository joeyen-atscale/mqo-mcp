//! [`Dataset`] — a minimal in-memory columnar table.
//!
//! Each column is a typed vector (`ColumnData`).  No Arrow dependency: the
//! layout is plain Rust enums + `Vec`, compact enough for the scan patterns
//! `dh-ops` needs while keeping compile times and binary size small.

use dh_spec::ColumnSchema;
use serde::{Deserialize, Serialize};

// ── Typed column storage ───────────────────────────────────────────────────

/// The typed values stored in a single column.
///
/// Variants mirror [`dh_spec::DType`]; each holds a contiguous `Vec` of
/// that type.  `Null` slots inside a nullable column are represented as
/// `None` in `Nullable*` variants (not a separate sentinel).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ColumnData {
    /// 64-bit signed integers.
    Int(Vec<Option<i64>>),
    /// 64-bit IEEE 754 floats.
    Float(Vec<Option<f64>>),
    /// Arbitrary-precision decimals stored as strings.
    Decimal(Vec<Option<String>>),
    /// UTF-8 strings.
    Str(Vec<Option<String>>),
    /// Booleans.
    Bool(Vec<Option<bool>>),
    /// Calendar dates (ISO 8601 string).
    Date(Vec<Option<String>>),
    /// Timestamps / times (ISO 8601 string).
    Time(Vec<Option<String>>),
}

impl ColumnData {
    /// Number of logical rows in this column.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Int(v) => v.len(),
            Self::Float(v) => v.len(),
            Self::Bool(v) => v.len(),
            Self::Decimal(v) | Self::Str(v) | Self::Date(v) | Self::Time(v) => v.len(),
        }
    }

    /// True when there are no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Approximate heap bytes consumed by this column.
    ///
    /// Counts only the heap allocation of the `Vec` itself plus string heap
    /// allocations; does not include stack overhead.
    #[must_use]
    pub fn byte_estimate(&self) -> usize {
        match self {
            Self::Int(v) => v.len() * std::mem::size_of::<Option<i64>>(),
            Self::Float(v) => v.len() * std::mem::size_of::<Option<f64>>(),
            Self::Bool(v) => v.len() * std::mem::size_of::<Option<bool>>(),
            Self::Decimal(v) | Self::Str(v) | Self::Date(v) | Self::Time(v) => v
                .iter()
                .map(|opt| {
                    std::mem::size_of::<Option<String>>()
                        + opt.as_ref().map_or(0, String::len)
                })
                .sum(),
        }
    }
}

// ── Dataset ────────────────────────────────────────────────────────────────

/// An immutable, in-memory columnar table.
///
/// `Dataset` is `Clone` so that [`crate::Store::get`] can return an owned
/// snapshot without lifetime entanglement with the internal `Mutex`.
/// Callers that want a *different* dataset must go through
/// [`crate::Store::derive`], which mints a new handle.
///
/// # Invariant
///
/// `columns.len() == data.len()` and every `ColumnData` has the same `len()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dataset {
    /// Schema of each column, in order.
    pub columns: Vec<ColumnSchema>,
    /// Per-column typed data, aligned with `columns`.
    pub data: Vec<ColumnData>,
}

impl Dataset {
    /// Construct a new `Dataset`, checking the alignment invariant.
    ///
    /// # Errors
    ///
    /// Returns `Err` with a description if `columns.len() != data.len()` or if
    /// any two `ColumnData` slices differ in length.
    pub fn new(columns: Vec<ColumnSchema>, data: Vec<ColumnData>) -> Result<Self, String> {
        if columns.len() != data.len() {
            return Err(format!(
                "columns.len() ({}) != data.len() ({})",
                columns.len(),
                data.len()
            ));
        }
        if data.len() > 1 {
            let row_count = data[0].len();
            for (i, col) in data.iter().enumerate().skip(1) {
                if col.len() != row_count {
                    return Err(format!(
                        "column {i} has {} rows but column 0 has {row_count}",
                        col.len()
                    ));
                }
            }
        }
        Ok(Self { columns, data })
    }

    /// Number of rows in the dataset.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.data.first().map_or(0, ColumnData::len)
    }

    /// Approximate heap bytes used by the column data (excludes struct overhead).
    #[must_use]
    pub fn byte_estimate(&self) -> usize {
        self.data.iter().map(ColumnData::byte_estimate).sum()
    }
}
