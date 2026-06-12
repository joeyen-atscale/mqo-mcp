//! Catalog types — mirrors the describe_model JSON schema used by
//! mcp-federated-catalog and AtScale's describe_model endpoint.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// A single measure entry in a describe_model JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measure {
    pub unique_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_string: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregation_type: Option<String>,
    /// Catch-all for additional fields.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// A single dimension entry in a describe_model JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dimension {
    pub unique_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format_string: Option<String>,
    /// Catch-all for additional fields.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Provenance block from federated catalog output (ignored during diff, preserved).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub cluster: String,
    pub model: String,
}

/// A model block from a describe_model JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub unique_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
    #[serde(default)]
    pub measures: Vec<Measure>,
    #[serde(default)]
    pub dimensions: Vec<Dimension>,
    /// Catch-all for additional fields.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// Top-level describe_model JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DescribeModel {
    #[serde(default)]
    pub models: Vec<Model>,
    /// Catch-all for any other top-level fields (e.g. federation provenance).
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

impl DescribeModel {
    /// Parse from a JSON string.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}
