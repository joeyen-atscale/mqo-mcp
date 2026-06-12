//! Core enrichment logic: attaches `column_group` sets to every catalog column.

use crate::bindings::FactBindings;
use crate::catalog::{CatalogColumn, CatalogSnapshot};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A catalog column annotated with its `column_group` membership set.
///
/// Every field from [`CatalogColumn`] is preserved byte-identical (FR4).
/// Only `column_group` is added.
// serde_json::Value in `extra` does not implement Eq.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnrichedColumn {
    /// Fully-qualified unique name, copied from input.
    pub unique_name: String,
    /// Human-readable label, copied from input.
    pub label: Option<String>,
    /// Column kind (`"measure"` or `"level"`), copied from input.
    pub kind: Option<String>,
    /// Hierarchy name (for levels), copied from input.
    pub hierarchy: Option<String>,
    /// Level name (for levels), copied from input.
    pub level: Option<String>,
    /// Whether this is calculated, copied from input.
    pub is_calc: Option<bool>,
    /// **Added field** — set of column-group identifiers this entity belongs to.
    ///
    /// Empty when no binding was found (FR5: entity is still present; reported
    /// in [`CoverageReport::unbound`]).
    pub column_group: BTreeSet<String>,
    /// Pass-through of any additional fields, copied byte-identical from input (FR4).
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

impl EnrichedColumn {
    /// Construct from a [`CatalogColumn`] plus a resolved column-group set.
    fn from_catalog(col: &CatalogColumn, groups: BTreeSet<String>) -> Self {
        Self {
            unique_name: col.unique_name.clone(),
            label: col.label.clone(),
            kind: col.kind.clone(),
            hierarchy: col.hierarchy.clone(),
            level: col.level.clone(),
            is_calc: col.is_calc,
            column_group: groups,
            extra: col.extra.clone(),
        }
    }
}

/// Coverage summary emitted alongside the enriched columns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoverageReport {
    /// Total number of columns in the input.
    pub total: usize,
    /// Number of columns that received at least one column-group tag.
    pub bound: usize,
    /// Number of columns with no binding (empty `column_group` set).
    pub unbound_count: usize,
    /// `unique_names` of unbound entities (FR5 — never silently omit).
    pub unbound: Vec<String>,
    /// Coverage fraction (0.0–1.0).
    pub coverage_pct: f64,
}

/// The enriched catalog: all input columns plus `column_group` tags, and a
/// coverage report.
///
/// Schema identifier: `enriched-catalog.v1`
// serde_json::Value in `extra` does not implement Eq.
#[allow(clippy::derive_partial_eq_without_eq)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnrichedCatalog {
    /// Schema identifier for versioned downstream consumption.
    pub schema: String,
    /// Optional catalog name, copied from input.
    pub catalog: Option<String>,
    /// Optional schema name from input.
    #[serde(rename = "db_schema")]
    pub db_schema: Option<String>,
    /// Enriched columns — one per input column, in input order.
    pub columns: Vec<EnrichedColumn>,
    /// Coverage summary.
    pub coverage: CoverageReport,
    /// Pass-through of any additional top-level fields from the input.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// Enrich every column in `catalog` with `column_group` tags derived from
/// `bindings`.
///
/// This is the primary public API of the crate. It is pure, total, and
/// deterministic:
/// - No panics — malformed or empty catalogs return an empty-columns result.
/// - No network or LLM calls.
/// - Every input field is preserved in the output (FR4).
/// - Columns with no binding carry `column_group: []` and are reported in
///   [`CoverageReport::unbound`] (FR5).
#[must_use]
pub fn enrich(catalog: &CatalogSnapshot, bindings: &FactBindings) -> EnrichedCatalog {
    let mut enriched_columns: Vec<EnrichedColumn> = Vec::with_capacity(catalog.columns.len());
    let mut unbound: Vec<String> = Vec::new();

    for col in &catalog.columns {
        let groups = resolve_groups(col, bindings);
        if groups.is_empty() {
            unbound.push(col.unique_name.clone());
        }
        enriched_columns.push(EnrichedColumn::from_catalog(col, groups));
    }

    let total = enriched_columns.len();
    let unbound_count = unbound.len();
    let bound = total.saturating_sub(unbound_count);
    let coverage_pct = if total == 0 {
        1.0 // vacuously 100% when there's nothing to bind
    } else {
        // usize → f64 has no infallible From impl; cast_precision_loss only matters
        // for values > 2^53, which no catalog will reach.
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let pct = bound as f64 / total as f64;
        pct
    };

    // Filter out the 'schema' and 'db_schema' keys from extra to avoid duplication
    let extra = catalog
        .extra
        .iter()
        .filter(|(k, _)| *k != "schema" && *k != "db_schema")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    EnrichedCatalog {
        schema: "enriched-catalog.v1".to_string(),
        catalog: catalog.catalog.clone(),
        db_schema: catalog.schema.clone(),
        columns: enriched_columns,
        coverage: CoverageReport {
            total,
            bound,
            unbound_count,
            unbound,
            coverage_pct,
        },
        extra,
    }
}

/// Resolve the column-group set for a single catalog column.
///
/// - Measures: looked up by `unique_name` in `bindings.measures`.
/// - Levels: looked up by `hierarchy` name in `bindings.hierarchies`.
/// - Unknown kind or missing key: empty set (entity will be reported as unbound).
fn resolve_groups(col: &CatalogColumn, bindings: &FactBindings) -> BTreeSet<String> {
    match col.kind.as_deref() {
        Some("measure") => bindings.groups_for_measure(&col.unique_name),
        Some("level") => col
            .hierarchy
            .as_deref()
            .map(|h| bindings.groups_for_hierarchy(h))
            .unwrap_or_default(),
        _ => BTreeSet::new(),
    }
}

#[cfg(test)]
#[allow(clippy::panic, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::catalog::CatalogSnapshot;

    fn make_catalog(columns: Vec<CatalogColumn>) -> CatalogSnapshot {
        CatalogSnapshot {
            columns,
            ..Default::default()
        }
    }

    fn measure(unique_name: &str) -> CatalogColumn {
        CatalogColumn {
            unique_name: unique_name.to_string(),
            kind: Some("measure".to_string()),
            ..Default::default()
        }
    }

    fn level(unique_name: &str, hierarchy: &str) -> CatalogColumn {
        CatalogColumn {
            unique_name: unique_name.to_string(),
            kind: Some("level".to_string()),
            hierarchy: Some(hierarchy.to_string()),
            ..Default::default()
        }
    }

    fn single_group(name: &str) -> BTreeSet<String> {
        std::iter::once(name.to_string()).collect()
    }

    #[test]
    fn measure_gets_bound_group() {
        let mut bindings = FactBindings {
            measures: BTreeMap::new(),
            hierarchies: BTreeMap::new(),
        };
        bindings
            .measures
            .insert("m.foo".to_string(), single_group("sales"));
        let catalog = make_catalog(vec![measure("m.foo")]);
        let enriched = enrich(&catalog, &bindings);
        if let Some(col) = enriched.columns.first() {
            assert_eq!(col.column_group, single_group("sales"));
        } else {
            panic!("expected one column in enriched output");
        }
    }

    #[test]
    fn level_gets_hierarchy_group() {
        let mut bindings = FactBindings {
            measures: BTreeMap::new(),
            hierarchies: BTreeMap::new(),
        };
        bindings
            .hierarchies
            .insert("h.bar".to_string(), single_group("inventory"));
        let catalog = make_catalog(vec![level("h.bar.[Level]", "h.bar")]);
        let enriched = enrich(&catalog, &bindings);
        if let Some(col) = enriched.columns.first() {
            assert_eq!(col.column_group, single_group("inventory"));
        } else {
            panic!("expected one column in enriched output");
        }
    }

    #[test]
    fn unbound_column_reported_not_dropped() {
        let bindings = FactBindings {
            measures: BTreeMap::new(),
            hierarchies: BTreeMap::new(),
        };
        let catalog = make_catalog(vec![measure("m.missing")]);
        let enriched = enrich(&catalog, &bindings);
        assert_eq!(enriched.columns.len(), 1, "column must not be dropped");
        if let Some(col) = enriched.columns.first() {
            assert!(col.column_group.is_empty(), "unbound column_group must be empty");
        } else {
            panic!("expected one column");
        }
        assert_eq!(enriched.coverage.unbound, vec!["m.missing".to_string()]);
    }

    #[test]
    fn empty_catalog_coverage_is_one() {
        let bindings = FactBindings {
            measures: BTreeMap::new(),
            hierarchies: BTreeMap::new(),
        };
        let catalog = make_catalog(vec![]);
        let enriched = enrich(&catalog, &bindings);
        // 1.0 exactly because we set it explicitly for empty catalogs (no float arithmetic)
        assert!(
            (enriched.coverage.coverage_pct - 1.0_f64).abs() < f64::EPSILON,
            "coverage_pct must be 1.0 for empty catalog"
        );
    }

    #[test]
    fn schema_field_is_enriched_catalog_v1() {
        let bindings = FactBindings {
            measures: BTreeMap::new(),
            hierarchies: BTreeMap::new(),
        };
        let catalog = make_catalog(vec![]);
        let enriched = enrich(&catalog, &bindings);
        assert_eq!(enriched.schema, "enriched-catalog.v1");
    }
}
