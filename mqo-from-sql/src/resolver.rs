//! Resolver: maps raw unique_names from parsed SQL back to catalog entries.
//!
//! Given a `CatalogSnapshot`, each name from SELECT is classified as a `measure`
//! and each name from GROUP BY is classified as a `dimension_level`.

use mqo_catalog_binder::catalog::{CatalogSnapshot, ColumnEntry};

use crate::error::ResolveError;

/// Classification of a resolved column.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedKind {
    Measure,
    DimensionLevel { hierarchy: String },
}

/// A fully-resolved column entry.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ResolvedColumn {
    /// The canonical unique_name from the catalog.
    pub unique_name: String,
    pub kind: ResolvedKind,
}

/// Resolve a single raw name against the catalog snapshot as a measure.
///
/// Resolution precedence:
/// 1. Exact `unique_name` match (case-insensitive).
/// 2. `label` match (case-insensitive).
///
/// # Errors
///
/// - [`ResolveError::UnknownName`] if no match.
/// - [`ResolveError::AmbiguousName`] if multiple matches.
pub fn resolve_measure<'a>(
    raw: &str,
    snapshot: &'a CatalogSnapshot,
) -> Result<&'a ColumnEntry, ResolveError> {
    let key = raw.to_lowercase();

    // Pass 1: exact unique_name (case-insensitive)
    let by_unique: Vec<&ColumnEntry> = snapshot
        .columns
        .iter()
        .filter(|c| c.kind == "measure" && c.unique_name.to_lowercase() == key)
        .collect();

    if by_unique.len() == 1 {
        return Ok(by_unique[0]);
    }
    if by_unique.len() > 1 {
        return Err(ResolveError::AmbiguousName(
            raw.to_string(),
            by_unique.iter().map(|c| c.unique_name.clone()).collect(),
        ));
    }

    // Pass 2: label match (case-insensitive)
    let by_label: Vec<&ColumnEntry> = snapshot
        .columns
        .iter()
        .filter(|c| c.kind == "measure" && c.label.to_lowercase() == key)
        .collect();

    match by_label.len() {
        1 => Ok(by_label[0]),
        0 => Err(ResolveError::UnknownName(raw.to_string())),
        _ => Err(ResolveError::AmbiguousName(
            raw.to_string(),
            by_label.iter().map(|c| c.unique_name.clone()).collect(),
        )),
    }
}

/// Resolve a single raw name against the catalog snapshot as a dimension level.
///
/// # Errors
///
/// - [`ResolveError::UnknownName`] if no match.
/// - [`ResolveError::AmbiguousName`] if multiple matches.
pub fn resolve_dimension_level<'a>(
    raw: &str,
    snapshot: &'a CatalogSnapshot,
) -> Result<&'a ColumnEntry, ResolveError> {
    let key = raw.to_lowercase();

    // Try exact unique_name match first
    let by_unique: Vec<&ColumnEntry> = snapshot
        .columns
        .iter()
        .filter(|c| c.kind == "level" && c.unique_name.to_lowercase() == key)
        .collect();

    if by_unique.len() == 1 {
        return Ok(by_unique[0]);
    }
    if by_unique.len() > 1 {
        return Err(ResolveError::AmbiguousName(
            raw.to_string(),
            by_unique.iter().map(|c| c.unique_name.clone()).collect(),
        ));
    }

    // Try label match
    let by_label: Vec<&ColumnEntry> = snapshot
        .columns
        .iter()
        .filter(|c| c.kind == "level" && c.label.to_lowercase() == key)
        .collect();

    match by_label.len() {
        1 => Ok(by_label[0]),
        0 => Err(ResolveError::UnknownName(raw.to_string())),
        _ => Err(ResolveError::AmbiguousName(
            raw.to_string(),
            by_label.iter().map(|c| c.unique_name.clone()).collect(),
        )),
    }
}

/// Resolve all names from a parsed projection.
///
/// Measures come from SELECT columns; dimensions from GROUP BY.
/// A name appearing in GROUP BY is classified as a dimension level;
/// a name appearing only in SELECT is classified as a measure.
///
/// Returns `(resolved_measures, resolved_dimensions)`.
///
/// # Errors
///
/// Any unknown or ambiguous name produces a `ResolveError`.
#[allow(dead_code)]
pub fn resolve_projection(
    select_names: &[String],
    group_by_names: &[String],
    where_cols: &[String],
    snapshot: &CatalogSnapshot,
) -> Result<(Vec<ResolvedColumn>, Vec<ResolvedColumn>, Vec<ResolvedColumn>), ResolveError> {
    // group_by_names are dimension levels
    let dim_set: std::collections::HashSet<&String> = group_by_names.iter().collect();

    let mut measures = Vec::new();
    let mut dims = Vec::new();

    // SELECT columns: if also in GROUP BY → dimension level; else → measure
    for name in select_names {
        if dim_set.contains(name) {
            let entry = resolve_dimension_level(name, snapshot)?;
            dims.push(ResolvedColumn {
                unique_name: entry.unique_name.clone(),
                kind: ResolvedKind::DimensionLevel {
                    hierarchy: entry.hierarchy.clone().unwrap_or_default(),
                },
            });
        } else {
            let entry = resolve_measure(name, snapshot)?;
            measures.push(ResolvedColumn {
                unique_name: entry.unique_name.clone(),
                kind: ResolvedKind::Measure,
            });
        }
    }

    // GROUP BY columns not in SELECT
    for name in group_by_names {
        if !select_names.contains(name) {
            let entry = resolve_dimension_level(name, snapshot)?;
            dims.push(ResolvedColumn {
                unique_name: entry.unique_name.clone(),
                kind: ResolvedKind::DimensionLevel {
                    hierarchy: entry.hierarchy.clone().unwrap_or_default(),
                },
            });
        }
    }

    // WHERE columns — resolve as either measure or dimension level (best-effort)
    let mut filter_cols = Vec::new();
    for name in where_cols {
        // Try measure first, then dimension level
        let resolved = resolve_measure(name, snapshot)
            .map(|e| ResolvedColumn {
                unique_name: e.unique_name.clone(),
                kind: ResolvedKind::Measure,
            })
            .or_else(|_| {
                resolve_dimension_level(name, snapshot).map(|e| ResolvedColumn {
                    unique_name: e.unique_name.clone(),
                    kind: ResolvedKind::DimensionLevel {
                        hierarchy: e.hierarchy.clone().unwrap_or_default(),
                    },
                })
            })?;
        filter_cols.push(resolved);
    }

    Ok((measures, dims, filter_cols))
}
