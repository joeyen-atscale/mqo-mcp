//! `mcp-cluster-registry` — typed TOML/JSON multi-cluster configuration registry.
//!
//! Defines the shared `ClusterEntry` / `ClusterRegistry` types consumed by all
//! AtScale federation tools.  No network, no async, no credential values —
//! only env-var *names* are stored (AC7).

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// All errors produced by this crate.
#[derive(Debug, Error, PartialEq)]
pub enum RegistryError {
    #[error("duplicate cluster name: {0}")]
    DuplicateName(String),

    #[error("cluster list is empty")]
    EmptyClusters,

    #[error("cluster '{name}' is missing required field '{field}'")]
    MissingRequiredField { name: String, field: String },

    #[error("invalid priority for cluster '{name}'")]
    InvalidPriority { name: String },

    #[error("TOML parse error: {0}")]
    TomlParse(String),

    #[error("JSON parse error: {0}")]
    JsonParse(String),
}

// ---------------------------------------------------------------------------
// AuthConfig
// ---------------------------------------------------------------------------

/// Authentication configuration stored per cluster.
///
/// **AC7 guarantee**: only env-var *names* (Strings) are stored and serialized.
/// The serializer NEVER calls `std::env::var()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthConfig {
    /// Direct PGWire credentials supplied via environment variables.
    Direct {
        /// Name of the env var holding the PGWire username.
        pg_user: String,
        /// Name of the env var holding the PGWire password.
        pg_pass_env: String,
    },
    /// OIDC client-credentials flow.
    Oidc {
        /// Token endpoint URL (not a secret — it's a public discovery URL).
        token_url: String,
        /// OIDC client ID (not a secret).
        client_id: String,
        /// OIDC realm name (not a secret).
        realm: String,
        /// Name of the env var holding the OIDC client secret.
        client_secret_env: String,
    },
}

// ---------------------------------------------------------------------------
// ClusterEntry
// ---------------------------------------------------------------------------

/// A single cluster in the registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClusterEntry {
    /// Short identifier, e.g. `"prod"` or `"staging"`.
    pub name: String,
    /// PGWire host:port, e.g. `"mcp-aws.atscaleinternal.com:15432"`.
    pub endpoint: String,
    /// Engine XMLA URL for MDX/DAX workloads (optional).
    pub xmla_url: Option<String>,
    /// Authentication configuration.
    pub auth: AuthConfig,
    /// Subset of `["sql", "dax", "mdx"]` this cluster supports.
    pub supported_backends: Vec<String>,
    /// If `Some`, only these model unique-names are routable to this cluster.
    pub model_filter: Option<Vec<String>>,
    /// Routing priority: lower value = higher preference.
    pub priority: u8,
    /// If `true`, a health failure brings the gateway down.
    pub required: bool,
    /// Optional labels, e.g. `["prod", "snowflake"]`.
    #[serde(default)]
    pub tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// ClusterRegistry
// ---------------------------------------------------------------------------

/// Top-level registry holding all cluster entries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClusterRegistry {
    pub clusters: Vec<ClusterEntry>,
}

impl ClusterRegistry {
    // ------------------------------------------------------------------
    // Constructors
    // ------------------------------------------------------------------

    /// Parse from a TOML string.
    pub fn from_toml(s: &str) -> Result<Self, RegistryError> {
        toml::from_str::<ClusterRegistry>(s).map_err(|e| RegistryError::TomlParse(e.to_string()))
    }

    /// Parse from a JSON string.
    pub fn from_json(s: &str) -> Result<Self, RegistryError> {
        serde_json::from_str::<ClusterRegistry>(s)
            .map_err(|e| RegistryError::JsonParse(e.to_string()))
    }

    // ------------------------------------------------------------------
    // Serialization
    // ------------------------------------------------------------------

    /// Serialize to a JSON string (pretty-printed).
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("ClusterRegistry is always serializable")
    }

    // ------------------------------------------------------------------
    // Query helpers
    // ------------------------------------------------------------------

    /// Look up a cluster by name.
    pub fn get(&self, name: &str) -> Option<&ClusterEntry> {
        self.clusters.iter().find(|c| c.name == name)
    }

    /// Return clusters sorted ascending by `priority` (0 = highest priority first).
    pub fn by_priority(&self) -> Vec<&ClusterEntry> {
        let mut sorted: Vec<&ClusterEntry> = self.clusters.iter().collect();
        sorted.sort_by_key(|c| c.priority);
        sorted
    }

    /// Does the named cluster declare support for `backend`?
    pub fn supports_backend(&self, cluster: &str, backend: &str) -> bool {
        self.get(cluster)
            .map(|c| c.supported_backends.iter().any(|b| b == backend))
            .unwrap_or(false)
    }

    /// Return clusters compatible with `model_name`: those whose `model_filter`
    /// is `None` (any model) OR contains `model_name`.
    pub fn clusters_for_model(&self, model_name: &str) -> Vec<&ClusterEntry> {
        self.clusters
            .iter()
            .filter(|c| match &c.model_filter {
                None => true,
                Some(filter) => filter.iter().any(|m| m == model_name),
            })
            .collect()
    }

    /// Return the highest-priority required cluster (lowest `priority` value
    /// among entries where `required == true`), or `None` if there are none.
    pub fn primary_cluster(&self) -> Option<&ClusterEntry> {
        self.clusters
            .iter()
            .filter(|c| c.required)
            .min_by_key(|c| c.priority)
    }

    // ------------------------------------------------------------------
    // Validation
    // ------------------------------------------------------------------

    /// Validate the registry, collecting all errors.
    ///
    /// Returns `Ok(())` when valid, or `Err(Vec<RegistryError>)` listing
    /// every problem found.
    pub fn validate(&self) -> Result<(), Vec<RegistryError>> {
        let mut errors: Vec<RegistryError> = Vec::new();

        if self.clusters.is_empty() {
            errors.push(RegistryError::EmptyClusters);
            return Err(errors);
        }

        // Duplicate-name check.
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for entry in &self.clusters {
            if !seen.insert(entry.name.as_str()) {
                errors.push(RegistryError::DuplicateName(entry.name.clone()));
            }
        }

        // Per-entry field checks.
        for entry in &self.clusters {
            if entry.endpoint.is_empty() {
                errors.push(RegistryError::MissingRequiredField {
                    name: entry.name.clone(),
                    field: "endpoint".to_string(),
                });
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}
