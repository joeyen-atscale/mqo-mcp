//! [`ExportReceipt`] — the audit record of a completed export.

use crate::types::{ExportDest, ExportFmt};
use dh_spec::DatasetHandle;
use serde::{Deserialize, Serialize};

/// The audit record produced by every successful [`crate::export`] call.
///
/// Carries everything needed to reconstruct provenance: which handle, what
/// format, where it went, how many rows, how many bytes, a content hash, and
/// when it happened.
///
/// `inline_payload` is `Some` only when the destination is
/// [`ExportDest::Inline`]; it is `None` for file exports (the bytes live on
/// disk at the path recorded in `dest`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportReceipt {
    /// The handle that was exported.
    pub handle: DatasetHandle,

    /// The format the data was serialized to.
    pub fmt: ExportFmt,

    /// The destination the data was delivered to.
    pub dest: ExportDest,

    /// Number of rows exported.
    pub row_count: u64,

    /// Number of bytes in the serialized payload.
    pub bytes: u64,

    /// Hex-encoded SHA-256 of the serialized payload.
    pub sha256: String,

    /// Unix timestamp (seconds) when the export completed.
    pub ts: i64,

    /// The raw payload bytes, present only for [`ExportDest::Inline`]
    /// destinations.
    ///
    /// Serialized as a base64 string in JSON; `None` for file exports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline_payload: Option<Vec<u8>>,
}
