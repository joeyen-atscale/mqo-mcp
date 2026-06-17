//! Catalog snapshot types — the JSON bundle from `list_models` / `search_columns` /
//! `describe_model` that the binder uses as its ground truth.

use serde::{Deserialize, Serialize};

/// A single column (measure or dimension level) from `search_columns`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ColumnEntry {
    /// Fully-qualified unique name (e.g. `"sales.revenue"`, `"time.calendar.[Year]"`).
    pub unique_name: String,

    /// Human-readable label (may not be unique across models).
    pub label: String,

    /// `"measure"` or `"level"`.
    pub kind: String,

    /// For `kind == "level"`: the hierarchy unique name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hierarchy: Option<String>,

    /// For `kind == "level"`: the level name within the hierarchy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,

    /// Present when the measure is semi-additive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semi_additive: Option<SemiAdditiveInfo>,

    /// Required dimension for this measure (R11 metadata).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_dimension: Option<String>,

    /// True when this is a calculated member (not stored aggregate).
    #[serde(default)]
    pub is_calc: bool,

    /// For `kind == "level"`: optional enumerated member domain from the
    /// level-domain capture probe (bounded at 1000). Present when the served
    /// catalog has been enriched; absent in live mode (skips conservatively).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<Vec<String>>,

    /// For `kind == "level"`: true distinct-member count from `LEVEL_CARDINALITY`
    /// as reported by `MDSCHEMA_LEVELS` at ingest time. Unlike `domain.len()` this
    /// is NOT capped by `domain_cap`, so levels whose domains are truncated (e.g.
    /// `Sold Calendar Week` with 10,436 distinct values) still carry the real
    /// count.  `None` when the cluster did not report a non-zero cardinality (old
    /// snapshot back-compat, or a level the cluster has no metadata for).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cardinality: Option<u64>,
}

/// Semi-additive metadata on a measure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemiAdditiveInfo {
    /// Hierarchies that trigger semi-additive behaviour (e.g. time hierarchy).
    pub trigger_hierarchies: Vec<String>,
}

/// The output of `describe_model` — specifically the Calculation Groups section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DescribeModelOutput {
    /// All calc-group members known for this model.
    pub calc_groups: Vec<CalcGroupEntry>,
}

/// One calc-group member from the `## Calculation Groups` section.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalcGroupEntry {
    /// The calculation group name (e.g. `"Time Intelligence"`).
    pub group_name: String,

    /// The member name within the group (e.g. `"YTD"`).
    pub member_name: String,

    /// The fully-qualified unique name for this member.
    pub unique_name: String,

    /// The MDX expression for this member.
    pub mdx: String,
}

/// The full catalog snapshot bundle consumed by `mqo-bind`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogSnapshot {
    /// `AtScale` catalog name (first path component for SQL FROM clause),
    /// e.g. `"atscale_catalogs"`. Required for fully-qualified SQL generation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,

    /// `AtScale` schema name (second path component for SQL FROM clause),
    /// e.g. `"tpcds_Snowflake"`. Required for fully-qualified SQL generation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,

    /// All columns (measures + levels) from `search_columns` / `list_models` outputs.
    pub columns: Vec<ColumnEntry>,

    /// Optional `describe_model` output (required when the MQO has `CalcGroupMember` filters).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub describe_model: Option<DescribeModelOutput>,
}
