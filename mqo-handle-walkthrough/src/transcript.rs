//! Transcript types and serialisation for the walkthrough artifact.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single turn in the walkthrough.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub turn: usize,
    pub op: String,
    pub input_handle: Option<String>,
    pub output_handle: String,
    pub row_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vega_lite_spec: Option<Value>,
}

/// Header metadata for the full walkthrough.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalkthroughHeader {
    pub requery_count: usize,
    pub store_backend: String,
    pub total_handles: usize,
}

/// The full transcript artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalkthroughTranscript {
    pub header: WalkthroughHeader,
    pub turns: Vec<TurnRecord>,
}
