//! `mqoguard-filter-bind-report` — reports which MQO member filters bound vs.
//! were silently dropped during compilation.
//!
//! # Overview
//!
//! When an agent puts a [`MemberFilter`] on an MQO — e.g.
//! `hierarchy = "sold_date_dimensions", members = ["2001"]` — and the
//! `AtScale` server cannot bind it, the filter is silently dropped: the
//! compiled SQL has no WHERE clause. This library makes filter binding
//! explicit.
//!
//! Call [`report_filters`] with a [`BoundMqo`] and the [`CompiledQuery`]
//! produced by the server. It returns a [`FilterBindReport`] that partitions
//! every filter into [`AppliedFilter`] (constraint present in the compiled
//! query) or [`DroppedFilter`] (constraint absent, with a typed [`DropReason`]).
//!
//! **Invariant (AC2 / FR3):** `applied ∪ dropped == input filter set`,
//! verified by the property tests in `tests/acceptance_AC2.rs`.
//!
//! # Detection strategy
//!
//! Detection is based on the compiled query (NFR2), not on a re-implementation
//! of binding logic. Two modes:
//!
//! - **Exact** — the [`CompiledQuery`] carries a `bound_filter_ids` list
//!   populated by the compiler. Membership in that list determines outcome.
//! - **Heuristic** — no binding record present; fall back to SQL text search
//!   for each member key. Confidence is marked [`DetectionConfidence::Heuristic`].
//!
//! # Panic safety
//!
//! This crate is `#![deny(unsafe_code)]`. Every public function is total:
//! it returns a typed result or a fully-`dropped` report rather than panicking
//! on malformed input (AC6).

#![deny(unsafe_code)]
#![warn(missing_docs)]
#![warn(rust_2018_idioms)]

use serde::{Deserialize, Serialize};

// ──────────────────────────────────────────────────────────────────────────────
// Input types
// ──────────────────────────────────────────────────────────────────────────────

/// A member filter as specified by the caller on an MQO.
///
/// A member filter constrains a hierarchy to the rows whose member keys appear
/// in [`members`](MemberFilter::members).  The [`filter_id`] is caller-assigned
/// and used to correlate filters in the output report; it must be unique within
/// one [`BoundMqo`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MemberFilter {
    /// Caller-assigned unique identifier for this filter within the MQO.
    pub filter_id: String,
    /// The hierarchy being filtered (e.g. `"sold_date_dimensions"`).
    pub hierarchy: String,
    /// Optional level within the hierarchy (e.g. `"Sold Calendar Year"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// The member keys to retain (e.g. `["2001"]`).
    pub members: Vec<String>,
}

/// An MQO filter — currently only member filters are supported.
///
/// The enum is `#[non_exhaustive]` so that future filter kinds (range, set,
/// expression) can be added without breaking existing match arms — unrecognised
/// variants always map to [`DropReason::UnsupportedFilterType`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MqoFilter {
    /// A member-inclusion filter.
    Member(MemberFilter),
}

impl MqoFilter {
    /// Returns the filter's unique identifier.
    #[must_use]
    pub fn filter_id(&self) -> &str {
        match self {
            Self::Member(f) => &f.filter_id,
        }
    }
}

/// The hierarchy/level catalog that the compiler knows about.
///
/// Used by [`report_filters`] to determine whether an unknown hierarchy or
/// level was the cause of a drop, and to surface member-key shape suggestions
/// (FR4 / AC4).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HierarchyCatalog {
    /// Known hierarchies, keyed by name.
    pub hierarchies: Vec<HierarchyMeta>,
}

/// Metadata for one hierarchy in the catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchyMeta {
    /// Hierarchy name (e.g. `"sold_date_dimensions"`).
    pub name: String,
    /// Levels defined under this hierarchy, in order from coarsest to finest.
    pub levels: Vec<LevelMeta>,
}

/// Metadata for one level within a hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LevelMeta {
    /// Level name (e.g. `"Sold Calendar Year"`).
    pub name: String,
    /// Human-readable description of the expected member-key shape for this
    /// level (e.g. `"four-digit integer year, e.g. '2001'"`).  Used to
    /// populate [`DroppedFilter::suggestion`] (FR4 / AC4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_key_shape: Option<String>,
}

/// A bound MQO: the set of filters the caller specified on the query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundMqo {
    /// The filters the caller requested.
    pub filters: Vec<MqoFilter>,
    /// Optional catalog used for drop-reason classification and suggestions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<HierarchyCatalog>,
}

/// The compiled query as produced by the `AtScale` server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompiledQuery {
    /// The SQL text emitted by the compiler.  May be empty if compilation
    /// failed before SQL generation.
    pub sql: String,
    /// When the compiler exposes its binding record, this list contains the
    /// `filter_id`s of every filter it successfully bound.  When `None` (the
    /// common case today), detection falls back to SQL text search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_filter_ids: Option<Vec<String>>,
}

// ──────────────────────────────────────────────────────────────────────────────
// Output types
// ──────────────────────────────────────────────────────────────────────────────

/// How confident we are that a binding-outcome determination is correct.
///
/// - [`Exact`](DetectionConfidence::Exact) — the compiler's own binding record
///   was present and used.
/// - [`Heuristic`](DetectionConfidence::Heuristic) — no binding record; outcome
///   inferred from SQL text search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionConfidence {
    /// The compiler's `bound_filter_ids` list was present and used.
    Exact,
    /// No compiler binding record; detection was by SQL text search.
    Heuristic,
}

/// Why a filter was dropped during compilation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum DropReason {
    /// The member key(s) could not be bound — the key format is wrong or the
    /// member does not exist at the target level.
    UnbindableMember,
    /// The hierarchy name in the filter does not exist in the semantic model.
    UnknownHierarchy,
    /// The level name in the filter does not exist within the hierarchy.
    UnknownLevel,
    /// The filter kind is not supported by the current version of this library
    /// or by the engine.
    UnsupportedFilterType,
}

/// A filter that was successfully applied: its constraint is present in the
/// compiled query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedFilter {
    /// The original filter specification.
    pub filter: MqoFilter,
    /// How this determination was made.
    pub confidence: DetectionConfidence,
}

/// A filter that was dropped: its constraint is absent from the compiled query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DroppedFilter {
    /// The original filter specification.
    pub filter: MqoFilter,
    /// Why the filter was dropped.
    pub reason: DropReason,
    /// How this determination was made.
    pub confidence: DetectionConfidence,
    /// Optional hint about the expected member-key shape, derived from the
    /// catalog when available (FR4 / AC4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// The complete binding report for one `query_multidimensional` call.
///
/// **Invariant:** `applied.len() + dropped.len() == input_filter_count`.
/// Every input filter appears in exactly one of the two lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterBindReport {
    /// Filters whose constraints are present in the compiled query.
    pub applied: Vec<AppliedFilter>,
    /// Filters whose constraints are absent from the compiled query.
    pub dropped: Vec<DroppedFilter>,
    /// The number of filters in the original MQO, for quick cross-checking.
    pub input_filter_count: usize,
    /// Detection mode used for this report.
    pub detection_mode: DetectionMode,
}

/// Which detection strategy was used to produce this report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionMode {
    /// Compiler binding record was present; all determinations are exact.
    ExactBindingRecord,
    /// No binding record; all determinations are heuristic (SQL text search).
    HeuristicSqlSearch,
}

// ──────────────────────────────────────────────────────────────────────────────
// Core logic
// ──────────────────────────────────────────────────────────────────────────────

/// Report which filters in `mqo` were applied vs. dropped by the compiler.
///
/// # Invariant
///
/// `result.applied.len() + result.dropped.len() == mqo.filters.len()`.
///
/// # Detection strategy
///
/// When `compiled.bound_filter_ids` is `Some`, it is the authoritative source
/// (exact confidence).  Otherwise, each filter's member keys are searched for
/// in `compiled.sql` (heuristic confidence).
///
/// # Errors
///
/// This function is infallible — it always returns a [`FilterBindReport`].
/// Malformed / empty inputs yield a fully-dropped report rather than a panic
/// (AC6, NFR1).
#[must_use]
pub fn report_filters(mqo: &BoundMqo, compiled: &CompiledQuery) -> FilterBindReport {
    let input_filter_count = mqo.filters.len();

    if input_filter_count == 0 {
        return FilterBindReport {
            applied: vec![],
            dropped: vec![],
            input_filter_count: 0,
            detection_mode: DetectionMode::HeuristicSqlSearch,
        };
    }

    compiled.bound_filter_ids.as_ref().map_or_else(
        || report_heuristic(mqo, compiled),
        |bound_ids| report_exact(mqo, compiled, bound_ids),
    )
}

/// Exact detection path: compiler's binding record is present.
fn report_exact(
    mqo: &BoundMqo,
    _compiled: &CompiledQuery,
    bound_ids: &[String],
) -> FilterBindReport {
    let mut applied = Vec::new();
    let mut dropped = Vec::new();

    for filter in &mqo.filters {
        let fid = filter.filter_id();
        if bound_ids.iter().any(|id| id == fid) {
            applied.push(AppliedFilter {
                filter: filter.clone(),
                confidence: DetectionConfidence::Exact,
            });
        } else {
            let (reason, suggestion) = classify_drop(filter, mqo.catalog.as_ref());
            dropped.push(DroppedFilter {
                filter: filter.clone(),
                reason,
                confidence: DetectionConfidence::Exact,
                suggestion,
            });
        }
    }

    FilterBindReport {
        input_filter_count: mqo.filters.len(),
        applied,
        dropped,
        detection_mode: DetectionMode::ExactBindingRecord,
    }
}

/// Heuristic detection path: SQL text search for member keys.
fn report_heuristic(mqo: &BoundMqo, compiled: &CompiledQuery) -> FilterBindReport {
    let sql_upper = compiled.sql.to_uppercase();
    let mut applied = Vec::new();
    let mut dropped = Vec::new();

    for filter in &mqo.filters {
        match filter {
            MqoFilter::Member(mf) => {
                // A member filter is considered applied if ALL member keys
                // appear somewhere in the SQL text (case-insensitive).  An
                // empty members list cannot bind anything — treat as dropped.
                let all_present = !mf.members.is_empty()
                    && mf
                        .members
                        .iter()
                        .all(|m| sql_upper.contains(&m.to_uppercase()));

                if all_present {
                    applied.push(AppliedFilter {
                        filter: filter.clone(),
                        confidence: DetectionConfidence::Heuristic,
                    });
                } else {
                    let (reason, suggestion) = classify_drop(filter, mqo.catalog.as_ref());
                    dropped.push(DroppedFilter {
                        filter: filter.clone(),
                        reason,
                        confidence: DetectionConfidence::Heuristic,
                        suggestion,
                    });
                }
            }
        }
    }

    FilterBindReport {
        input_filter_count: mqo.filters.len(),
        applied,
        dropped,
        detection_mode: DetectionMode::HeuristicSqlSearch,
    }
}

/// Classify why a filter was dropped and surface a catalog suggestion if
/// available (FR4 / AC4).
fn classify_drop(
    filter: &MqoFilter,
    catalog: Option<&HierarchyCatalog>,
) -> (DropReason, Option<String>) {
    match filter {
        MqoFilter::Member(mf) => {
            let Some(cat) = catalog else {
                // No catalog → can only say the member was unbindable.
                return (DropReason::UnbindableMember, None);
            };

            // Check whether the hierarchy is known.
            let Some(hier) = cat.hierarchies.iter().find(|h| h.name == mf.hierarchy) else {
                return (DropReason::UnknownHierarchy, None);
            };

            // If a level was specified, check whether it exists.
            if let Some(level_name) = &mf.level {
                let Some(level) = hier.levels.iter().find(|l| l.name == *level_name) else {
                    return (DropReason::UnknownLevel, None);
                };
                // Level found; suggest the expected key shape if available.
                let suggestion = level.expected_key_shape.clone().map(|shape| {
                    format!(
                        "Level '{level_name}' in hierarchy '{}' expects: {shape}",
                        mf.hierarchy
                    )
                });
                return (DropReason::UnbindableMember, suggestion);
            }

            // No level specified; look for a suggestion from any level.
            let suggestion = hier.levels.first().and_then(|l| {
                l.expected_key_shape.as_ref().map(|shape| {
                    format!(
                        "Hierarchy '{}' first level '{}' expects: {shape}",
                        mf.hierarchy, l.name
                    )
                })
            });

            (DropReason::UnbindableMember, suggestion)
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Convenience: build a BoundMqo / CompiledQuery from serde_json::Value
// ──────────────────────────────────────────────────────────────────────────────

/// Parse a [`BoundMqo`] from a JSON value.
///
/// Returns an empty-filter [`BoundMqo`] on parse failure rather than
/// propagating the error, because [`report_filters`] must not panic (NFR1).
/// Callers who need the parse error should use `serde_json::from_value` directly.
///
/// # Errors
///
/// Returns a [`serde_json::Error`] when the value does not match the schema.
pub fn bound_mqo_from_value(value: serde_json::Value) -> Result<BoundMqo, serde_json::Error> {
    serde_json::from_value(value)
}

/// Parse a [`CompiledQuery`] from a JSON value.
///
/// # Errors
///
/// Returns a [`serde_json::Error`] when the value does not match the schema.
pub fn compiled_query_from_value(
    value: serde_json::Value,
) -> Result<CompiledQuery, serde_json::Error> {
    serde_json::from_value(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member_filter(id: &str, hierarchy: &str, members: &[&str]) -> MqoFilter {
        MqoFilter::Member(MemberFilter {
            filter_id: id.to_owned(),
            hierarchy: hierarchy.to_owned(),
            level: None,
            members: members.iter().map(|s| (*s).to_owned()).collect(),
        })
    }

    /// Verify the applied∪dropped == input invariant on a trivial case.
    #[test]
    fn invariant_holds_simple() {
        let mqo = BoundMqo {
            filters: vec![
                member_filter("f1", "sold_date_dimensions", &["2001"]),
                member_filter("f2", "store_dim", &["store_42"]),
            ],
            catalog: None,
        };
        let compiled = CompiledQuery {
            sql: "SELECT SUM(ss_net_profit) FROM tpcds WHERE store_id = 'store_42'".to_owned(),
            bound_filter_ids: None,
        };
        let report = report_filters(&mqo, &compiled);
        assert_eq!(
            report.applied.len() + report.dropped.len(),
            report.input_filter_count
        );
    }

    /// Verify empty MQO returns empty report without panic.
    #[test]
    fn empty_mqo_ok() {
        let mqo = BoundMqo {
            filters: vec![],
            catalog: None,
        };
        let compiled = CompiledQuery {
            sql: String::new(),
            bound_filter_ids: None,
        };
        let report = report_filters(&mqo, &compiled);
        assert_eq!(report.input_filter_count, 0);
        assert!(report.applied.is_empty());
        assert!(report.dropped.is_empty());
    }

    /// Empty SQL → all filters dropped (heuristic).
    #[test]
    fn empty_sql_drops_all() {
        let mqo = BoundMqo {
            filters: vec![member_filter("f1", "sold_date_dimensions", &["2001"])],
            catalog: None,
        };
        let compiled = CompiledQuery {
            sql: String::new(),
            bound_filter_ids: None,
        };
        let report = report_filters(&mqo, &compiled);
        assert!(report.applied.is_empty());
        assert_eq!(report.dropped.len(), 1);
    }
}
