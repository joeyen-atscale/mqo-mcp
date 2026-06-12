//! Public-facing type enums for export format and destination.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The output format for an export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExportFmt {
    /// Comma-separated values (always available).
    Csv,

    /// JSON array of row objects, bounded by `max_rows`.
    ///
    /// If the dataset exceeds `max_rows` the export is refused with
    /// [`ExportError::JsonLimitExceeded`] unless
    /// [`ExportOptions::override_json_limit`] is set.
    Json {
        /// Maximum number of rows to allow in the JSON output.
        max_rows: usize,
    },

    /// Apache Parquet (requires the `parquet` cargo feature).
    Parquet,
}

/// Where the exported bytes are delivered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ExportDest {
    /// Write to a file at `path`.
    ///
    /// The write is atomic (tempfile + rename on POSIX).  Refuses to
    /// overwrite an existing file unless [`ExportOptions::overwrite`] is
    /// `true`.
    File(PathBuf),

    /// Return the bytes inline (inside [`ExportReceipt::inline_payload`]).
    ///
    /// The total payload must not exceed `max_bytes`; otherwise
    /// [`ExportError::InlineLimitExceeded`] is returned.
    Inline {
        /// Maximum payload size in bytes.
        max_bytes: usize,
    },
}

/// Extra options that modify export behaviour.
///
/// All fields default to `false`/off so callers only need to set the
/// fields relevant to their use case.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExportOptions {
    /// Allow overwriting an existing file when the destination is
    /// [`ExportDest::File`].
    pub overwrite: bool,

    /// Allow the JSON export to exceed `max_rows` specified in
    /// [`ExportFmt::Json`].  Intended for explicit operator overrides only.
    pub override_json_limit: bool,
}
