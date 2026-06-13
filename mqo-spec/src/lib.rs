//! # mqo-spec
//!
//! Multidimensional Query Object — the typed, `serde`-serializable schema and JSON Schema
//! contract shared by every component of the MQO fleet: binder, compilers, router, server,
//! and benchmark.
//!
//! This crate provides **only the shape and its validation** — no query logic.
//!
//! ## Quick start
//!
//! ```rust
//! use mqo_spec::{Mqo, MeasureRef, validate};
//!
//! let mqo = Mqo {
//!     model: "sales".to_string(),
//!     measures: vec![MeasureRef { unique_name: "sales.revenue".to_string() }],
//!     dimensions: vec![],
//!     filters: vec![],
//!     time_intelligence: vec![],
//!     order: None,
//!     limit: Some(100),
//!     non_empty: true,
//! };
//!
//! let result = validate(&mqo);
//! assert!(result.is_ok());
//! ```

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ── Core MQO struct ────────────────────────────────────────────────────────

/// The top-level Multidimensional Query Object.
///
/// An LLM constructs this *instead of SQL*. It is a selection of measures,
/// dimension levels, filters, calc-group members, and time-intelligence
/// operations, with optional ordering, limit, and non-empty flags.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Multidimensional Query Object — top-level query payload")]
pub struct Mqo {
    /// The model (cube) this query runs against.
    pub model: String,

    /// Measures to project. Must be non-empty.
    pub measures: Vec<MeasureRef>,

    /// Dimension levels to project (rows/columns).
    pub dimensions: Vec<LevelSelection>,

    /// Filters to apply.
    pub filters: Vec<Filter>,

    /// Time-intelligence operations to apply.
    pub time_intelligence: Vec<TimeIntel>,

    /// Optional ordering of result rows.
    pub order: Option<Vec<OrderKey>>,

    /// Optional row limit. Must be ≥ 1 if present.
    pub limit: Option<u64>,

    /// If true, exclude tuples where all measures are empty/null.
    pub non_empty: bool,
}

// ── Reference types ────────────────────────────────────────────────────────

/// A reference to a measure by its unique name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeasureRef {
    /// The unique name of the measure (e.g. `"sales.revenue"`).
    pub unique_name: String,
}

/// A dimension level selected for projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LevelSelection {
    /// The unique name of the hierarchy this level belongs to.
    pub hierarchy: String,

    /// The level within the hierarchy (e.g. `"year"`, `"month"`, `"day"`).
    pub level: String,
}

/// A single ordering key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OrderKey {
    /// The measure or dimension unique name to sort by.
    pub key: String,

    /// Sort direction.
    pub direction: SortDirection,
}

/// Sort direction for an [`OrderKey`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    Asc,
    Desc,
}

// ── RangeBound ────────────────────────────────────────────────────────────

/// A typed bound for a `Range` filter (PRD-mqo-range-filter-native-typing).
///
/// `#[serde(untagged)]` means a bare JSON number still deserializes as
/// `Number(f64)` (backward-compatible); an ISO-8601 date string deserializes as
/// `IsoDate`; any other string as `Text`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RangeBound {
    /// A numeric bound (the pre-existing form; bare JSON numbers land here).
    Number(f64),
    /// An ISO-8601 date bound or string bound for non-numeric levels.
    Text(String),
}

impl RangeBound {
    /// True when this bound is strictly greater than `other` within the same type.
    pub fn gt_bound(&self, other: &Self) -> bool {
        match (self, other) {
            (RangeBound::Number(a), RangeBound::Number(b)) => a > b,
            (RangeBound::Text(a), RangeBound::Text(b)) => a > b,
            _ => false,
        }
    }
    /// Extract the numeric value when this is a `Number` bound.
    pub fn as_f64(&self) -> Option<f64> {
        if let RangeBound::Number(n) = self { Some(*n) } else { None }
    }
    /// Extract the string value when this is a `Text` bound.
    pub fn as_str(&self) -> Option<&str> {
        if let RangeBound::Text(s) = self { Some(s.as_str()) } else { None }
    }
}

// ── Filter enum ────────────────────────────────────────────────────────────

/// A filter constraint applied to the query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Filter {
    /// Include only the listed member keys within a hierarchy.
    Member {
        /// The hierarchy to filter on.
        hierarchy: String,
        /// The member keys to include (strings; model-specific format).
        members: Vec<String>,
    },

    /// Include only the listed member keys within an **explicitly named level**
    /// of a hierarchy (PRD-mqo-member-filter-explicit-level). Unlike `Member`,
    /// which lets the compiler ground the value to a level by domain scan, this
    /// pins the level directly — required to disambiguate a member that appears
    /// in more than one level of the hierarchy (e.g. "M" ∈ Gender ∧ Marital
    /// Status). `exclude: true` means NOT-IN.
    MemberLevel {
        /// The hierarchy to filter on.
        hierarchy: String,
        /// The fully-qualified level unique_name (e.g. `customer_demographics.[Gender]`).
        level: String,
        /// The member keys.
        members: Vec<String>,
        /// When true, exclude these members (NOT-IN) instead of including them.
        #[serde(default)]
        exclude: bool,
    },

    /// Include only tuples where a level's value falls in `[lo, hi]`.
    Range {
        /// The level to filter on.
        level: String,
        /// Inclusive lower bound. A bare JSON number deserializes as `Number(f64)`
        /// (backward-compatible). ISO-8601 date strings (e.g. `"2002-05-27"`) are
        /// accepted for date-typed levels (PRD-mqo-range-filter-native-typing).
        lo: RangeBound,
        /// Inclusive upper bound. Must be ≥ `lo` within its type.
        hi: RangeBound,
    },

    /// Include only the named calculation-group member.
    CalcGroupMember {
        /// The calculation group name.
        calc_group: String,
        /// The specific member within the calculation group.
        member: String,
    },

    /// Boolean combination of leaf filters (PRD-mqo-filter-predicate-grammar).
    ///
    /// Supports one level of nesting: an OR of AND-groups, or an AND of OR-groups.
    /// Each element of `filters` must be a leaf predicate (`Member`, `MemberLevel`,
    /// `Range`) — nested `Group` variants are rejected with `UnsupportedFilterShape`.
    Group {
        /// Logical operator combining the elements of `filters`.
        op: FilterGroupOp,
        /// The leaf predicates to combine. Must be non-empty.
        filters: Vec<Filter>,
    },
}

/// Logical operator for a [`Filter::Group`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum FilterGroupOp {
    And,
    Or,
}

// ── TimeIntel enum ─────────────────────────────────────────────────────────

/// A time-intelligence operation to apply to measure results.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum TimeIntel {
    /// Year-over-year comparison.
    YoY,

    /// Prior-period comparison (one period back at the current grain).
    PriorPeriod,

    /// Cumulative measure from the start of the specified grain.
    ToDate {
        /// The grain at which to reset the accumulation.
        grain: Grain,
    },

    /// Running total across the result set, never reset.
    RunningTotal,

    /// Share of the measure relative to a parent level.
    Share {
        /// The level at which to compute the denominator.
        of_level: String,
    },

    /// Rank of each tuple by a specified measure.
    Rank {
        /// The measure unique name to rank by.
        by: String,
        /// Return only the top N ranked tuples.
        top_n: Option<u32>,
    },
}

/// Time grain used in [`TimeIntel::ToDate`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Grain {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

// ── BoundMqo ───────────────────────────────────────────────────────────────

/// The binder's output: a resolved MQO with fully-qualified `unique_name`s
/// and per-reference metadata flags.
///
/// This is a **type stub** — the binder implementation lives in a separate crate
/// that depends on `mqo-spec`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BoundMqo {
    /// The original MQO as submitted.
    pub mqo: Mqo,

    /// Resolved measure bindings.
    pub measures: Vec<BoundMeasure>,

    /// Resolved dimension bindings.
    pub dimensions: Vec<BoundDimension>,
}

/// A resolved reference to a measure with binder-supplied metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BoundMeasure {
    /// The fully-qualified unique name as resolved by the binder.
    pub unique_name: String,

    /// True when this measure is a calculated member (not a stored aggregate).
    pub is_calc: bool,

    /// True when this measure is semi-additive (e.g. balance, headcount).
    pub semi_additive: bool,

    /// A dimension level that must be present in the query for this measure
    /// to return correct results (e.g. an account-type dimension for
    /// last-balance measures).
    pub required_dimension: Option<String>,
}

/// A resolved reference to a dimension level with binder-supplied metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BoundDimension {
    /// The fully-qualified unique name of the level as resolved by the binder.
    pub unique_name: String,

    /// The hierarchy this level belongs to (resolved).
    pub hierarchy: String,
}

// ── Validation ─────────────────────────────────────────────────────────────

/// A structural validation error for an [`Mqo`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum MqoError {
    /// The `measures` array was empty.
    #[error("mqo.measures must not be empty")]
    EmptyMeasures,

    /// A `limit` of 0 is not meaningful.
    #[error("mqo.limit must be ≥ 1 when present, got 0")]
    LimitZero,

    /// A `Range` filter has `lo > hi` (within the same bound type).
    #[error("Range filter lo > hi on level '{level}'")]
    RangeLoGtHi { level: String },
}

/// Perform structural validation of an [`Mqo`].
///
/// Returns `Ok(())` if the MQO is structurally valid, or a list of all
/// [`MqoError`]s found. This is **not** semantic validation — it does not
/// check whether measure or dimension names exist in any model.
///
/// # Errors
///
/// Returns `Err(errors)` when one or more structural constraints are violated:
/// - [`MqoError::EmptyMeasures`] — `measures` is empty.
/// - [`MqoError::LimitZero`] — `limit` is `Some(0)`.
/// - [`MqoError::RangeLoGtHi`] — a `Range` filter has `lo > hi`.
pub fn validate(mqo: &Mqo) -> Result<(), Vec<MqoError>> {
    let mut errors = Vec::new();

    if mqo.measures.is_empty() {
        errors.push(MqoError::EmptyMeasures);
    }

    if mqo.limit == Some(0) {
        errors.push(MqoError::LimitZero);
    }

    for filter in &mqo.filters {
        if let Filter::Range { level, lo, hi } = filter {
            if lo.gt_bound(hi) {
                errors.push(MqoError::RangeLoGtHi { level: level.clone() });
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Emit the JSON Schema for [`Mqo`] as a pretty-printed JSON string.
///
/// The schema is derived via [`schemars::schema_for!`] and can be used by
/// non-Rust producers (e.g. the LLM skill, other languages) to validate MQO
/// payloads before sending them to the fleet.
///
/// # Panics
///
/// Panics only if `serde_json` fails to serialize the schema — this cannot
/// happen in practice for a schema derived from a concrete struct.
#[must_use]
pub fn emit_json_schema() -> String {
    let schema = schemars::schema_for!(Mqo);
    serde_json::to_string_pretty(&schema).expect("schemars schema is always serializable")
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    fn minimal_mqo() -> Mqo {
        Mqo {
            model: "sales".to_string(),
            measures: vec![MeasureRef {
                unique_name: "sales.revenue".to_string(),
            }],
            dimensions: vec![],
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
        }
    }

    #[test]
    fn validate_ok_on_minimal() {
        assert!(validate(&minimal_mqo()).is_ok());
    }

    #[test]
    fn validate_rejects_empty_measures() {
        let mut mqo = minimal_mqo();
        mqo.measures.clear();
        let errs = validate(&mqo).unwrap_err();
        assert!(errs.iter().any(|e| matches!(e, MqoError::EmptyMeasures)));
    }

    #[test]
    fn validate_rejects_limit_zero() {
        let mut mqo = minimal_mqo();
        mqo.limit = Some(0);
        let errs = validate(&mqo).unwrap_err();
        assert!(errs.iter().any(|e| matches!(e, MqoError::LimitZero)));
    }

    #[test]
    fn validate_rejects_range_lo_gt_hi() {
        let mut mqo = minimal_mqo();
        mqo.filters.push(Filter::Range {
            level: "year".to_string(),
            lo: RangeBound::Number(2024.0),
            hi: RangeBound::Number(2020.0),
        });
        let errs = validate(&mqo).unwrap_err();
        assert!(errs
            .iter()
            .any(|e| matches!(e, MqoError::RangeLoGtHi { .. })));
    }

    #[test]
    fn validate_range_lo_eq_hi_is_ok() {
        let mut mqo = minimal_mqo();
        mqo.filters.push(Filter::Range {
            level: "year".to_string(),
            lo: RangeBound::Number(2024.0),
            hi: RangeBound::Number(2024.0),
        });
        assert!(validate(&mqo).is_ok());
    }

    #[test]
    fn validate_collects_all_errors() {
        let mqo = Mqo {
            model: "sales".to_string(),
            measures: vec![],
            dimensions: vec![],
            filters: vec![Filter::Range {
                level: "year".to_string(),
                lo: RangeBound::Number(2025.0),
                hi: RangeBound::Number(2020.0),
            }],
            time_intelligence: vec![],
            order: None,
            limit: Some(0),
            non_empty: false,
        };
        let errs = validate(&mqo).unwrap_err();
        assert_eq!(errs.len(), 3);
    }

    #[test]
    fn emit_json_schema_is_valid_json() {
        let schema_str = emit_json_schema();
        let parsed: serde_json::Value =
            serde_json::from_str(&schema_str).expect("schema is valid JSON");
        assert!(parsed.is_object());
    }
}
