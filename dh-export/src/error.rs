//! Error types for `dh-export`.

use std::fmt;
use std::path::PathBuf;

/// All failure modes for [`crate::export`].
#[derive(Debug)]
#[non_exhaustive]
pub enum ExportError {
    /// The handle could not be resolved (not found, expired, or evicted).
    LookupFailed(String),

    /// The dataset has more rows than the `max_rows` limit in
    /// [`crate::ExportFmt::Json`] and no override was set.
    JsonLimitExceeded {
        /// Actual row count of the dataset.
        actual: usize,
        /// The `max_rows` limit that was exceeded.
        limit: usize,
    },

    /// The serialized payload exceeds `max_bytes` for an
    /// [`crate::ExportDest::Inline`] destination.
    InlineLimitExceeded {
        /// Actual byte count of the serialized payload.
        actual: usize,
        /// The `max_bytes` limit that was exceeded.
        limit: usize,
    },

    /// A file already exists at the target path and
    /// [`crate::ExportOptions::overwrite`] was not set.
    FileExists(PathBuf),

    /// An I/O error occurred during serialization or file writing.
    Io(String),

    /// The `parquet` cargo feature is not enabled.
    ParquetNotEnabled,
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LookupFailed(msg) => write!(f, "handle lookup failed: {msg}"),
            Self::JsonLimitExceeded { actual, limit } => write!(
                f,
                "JSON export refused: dataset has {actual} rows but limit is {limit}; \
                 set override_json_limit to export anyway"
            ),
            Self::InlineLimitExceeded { actual, limit } => write!(
                f,
                "inline export refused: payload is {actual} bytes but max_bytes is {limit}"
            ),
            Self::FileExists(path) => write!(
                f,
                "file already exists at {} and overwrite flag is not set",
                path.display()
            ),
            Self::Io(msg) => write!(f, "I/O error: {msg}"),
            Self::ParquetNotEnabled => write!(
                f,
                "Parquet export requires the `parquet` cargo feature; \
                 rebuild with `--features parquet`"
            ),
        }
    }
}

impl std::error::Error for ExportError {}
