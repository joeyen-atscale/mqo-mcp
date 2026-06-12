//! Local deserialization types for the `BoundMqo` JSON.
//!
//! These accept both the `mqo_spec::BoundMqo` shape and the extended
//! `BoundMqoOutput` shape emitted by `mqo-bind`, which adds
//! `trigger_hierarchies` and `calc_group_members`.

use mqo_spec::Mqo;
use serde::{Deserialize, Serialize};

/// Top-level deserialized form of the JSON produced by `mqo-bind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundMqoInput {
    /// The original MQO echoed back by the binder.
    pub mqo: Mqo,

    /// Resolved measure bindings.
    pub measures: Vec<BoundMeasureInput>,

    /// Resolved dimension bindings.
    pub dimensions: Vec<BoundDimensionInput>,

    /// Resolved calc-group member bindings (absent in the mqo-spec shape).
    #[serde(default)]
    pub calc_group_members: Vec<CalcGroupMemberInput>,
}

/// A resolved measure with binder metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundMeasureInput {
    /// Fully-qualified unique name as resolved by the binder.
    pub unique_name: String,

    /// True when this is a calculated member.
    #[serde(default)]
    pub is_calc: bool,

    /// True when this is semi-additive.
    #[serde(default)]
    pub semi_additive: bool,

    /// Optional required dimension for semi-additive measures.
    #[serde(default)]
    pub required_dimension: Option<String>,

    /// Semi-additive trigger hierarchies (binder-extended field).
    #[serde(default)]
    pub trigger_hierarchies: Vec<String>,
}

/// A resolved dimension level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundDimensionInput {
    /// Fully-qualified unique name of the level.
    pub unique_name: String,

    /// The hierarchy this level belongs to.
    pub hierarchy: String,
}

/// A resolved calc-group member from the binder's extended output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalcGroupMemberInput {
    /// Calc-group name (e.g. `"Time Intelligence"`).
    pub calc_group: String,
    /// Member name (e.g. `"YTD"`).
    pub member: String,
    /// Unique name for the calc-group entry.
    pub unique_name: String,
    /// MDX expression (carried through; not used in DAX path).
    #[serde(default)]
    pub mdx: String,
}
