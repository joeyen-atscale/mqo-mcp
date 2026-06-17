//! Output types for `mqo-pg-query`.
//!
//! Matches the Python oracle's `ReferenceTable` / `Oversize` shapes so
//! PRD-mqoeval-gold-via-cli can parse them directly.
//!
//! JSON schema:
//! - Success:  `{"columns": ["c1", ...], "rows": [[v, ...], ...]}`
//! - Oversize: `{"oversize": {"observed_at_least": N, "cap": C}}`
//! - Error:    `{"error": {"message": "..."}}`

use serde::Serialize;

use crate::TypedCell;

/// The typed result table: column headers + typed rows.
#[derive(Debug, Clone, Serialize)]
pub struct ReferenceTable {
    /// Column names in order.
    pub columns: Vec<String>,
    /// Typed rows (each row is ordered by `columns`).
    pub rows: Vec<Vec<TypedCell>>,
}

/// Payload inside `{"oversize": {...}}`.
#[derive(Debug, Clone, Serialize)]
pub struct OversizePayload {
    pub observed_at_least: usize,
    pub cap: usize,
}

/// Payload inside `{"error": {...}}`.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorPayload {
    pub message: String,
}

/// Wrapper to serialise as `{"oversize": {...}}`.
#[derive(Debug, Clone, Serialize)]
struct OversizeWrapper {
    oversize: OversizePayload,
}

/// Wrapper to serialise as `{"error": {...}}`.
#[derive(Debug, Clone, Serialize)]
struct ErrorWrapper {
    error: ErrorPayload,
}

/// The top-level output of a `mqo-pg-query` run.
///
/// Serialises to one of the three canonical shapes defined in the PRD.
#[derive(Debug, Clone)]
pub enum QueryOutput {
    /// Success: typed result table (may be empty, per FR5).
    Table(ReferenceTable),
    /// Result exceeded the row cap; full set was NOT streamed (FR3).
    Oversize {
        observed_at_least: usize,
        cap: usize,
    },
    /// Structured error (FR4).
    Error { message: String },
}

impl QueryOutput {
    /// Construct an oversize result.
    #[must_use]
    pub fn oversize(observed_at_least: usize, cap: usize) -> Self {
        QueryOutput::Oversize { observed_at_least, cap }
    }

    /// Construct an error result.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        QueryOutput::Error { message: message.into() }
    }

    /// Whether this output represents a non-error outcome.
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self, QueryOutput::Table(_) | QueryOutput::Oversize { .. })
    }

    /// Serialise to a JSON [`serde_json::Value`].
    ///
    /// # Panics
    ///
    /// Panics only if the internal types cannot be serialised, which should never
    /// happen for the bounded types used here.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            QueryOutput::Table(t) => {
                serde_json::to_value(t).expect("ReferenceTable serializes")
            }
            QueryOutput::Oversize { observed_at_least, cap } => {
                let w = OversizeWrapper {
                    oversize: OversizePayload {
                        observed_at_least: *observed_at_least,
                        cap: *cap,
                    },
                };
                serde_json::to_value(w).expect("OversizeWrapper serializes")
            }
            QueryOutput::Error { message } => {
                let w = ErrorWrapper {
                    error: ErrorPayload { message: message.clone() },
                };
                serde_json::to_value(w).expect("ErrorWrapper serializes")
            }
        }
    }
}
