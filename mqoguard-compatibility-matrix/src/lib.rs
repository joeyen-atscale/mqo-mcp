//! `mqoguard-compatibility-matrix` — compute measure×hierarchy compatibility
//! from an [`EnrichedCatalog`] and emit as a `compatibility-matrix.v1` JSON fragment.
//!
//! # Overview
//!
//! A (measure, hierarchy) pair is **compatible** iff the measure's
//! `column_group` set intersects the union of column-group sets of that
//! hierarchy's levels.
//!
//! The resulting [`CompatibilityMatrix`] is:
//! - **Symmetric**: measure M lists hierarchy H iff H lists measure M.
//! - **Deterministic**: pure function, no I/O, no randomness.
//! - **Fail-safe**: when no `column_group` data is present, returns an empty
//!   matrix with a diagnostic note rather than an all-compatible matrix.
//!
//! # Stub note
//!
//! The [`EnrichedCatalog`] and related types are defined inline here.
//! // TODO: replace with mqoguard-column-group-enrichment dep once published

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// EnrichedCatalog stub (inline until mqoguard-column-group-enrichment ships)
// TODO: replace with mqoguard-column-group-enrichment dep once published
// ---------------------------------------------------------------------------

/// A single column from the enriched `describe_model` payload.
///
/// Matches `enriched-catalog.v1` schema emitted by `mqoguard-column-group-enrichment`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnrichedColumn {
    /// Stable unique identifier for this column within the model.
    pub unique_name: String,
    /// Human-readable label.
    pub label: String,
    /// Column kind: `"measure"`, `"dimension"`, or other.
    pub kind: String,
    /// Whether this column is a calculated member.
    #[serde(default)]
    pub is_calc: bool,
    /// The hierarchy this level belongs to (for dimension columns).
    /// `None` for measures.
    #[serde(default)]
    pub hierarchy: Option<String>,
    /// The level name within the hierarchy (for dimension columns).
    #[serde(default)]
    pub level: Option<String>,
    /// Fact/subject-area column-group tags.
    ///
    /// Set of column-group identifiers this entity belongs to.
    /// Empty set means "no binding found" (fail-safe: not all-compatible).
    ///
    /// Added by `mqoguard-column-group-enrichment`; absent in raw catalog.
    #[serde(default)]
    pub column_group: BTreeSet<String>,
}

/// A snapshot of a `describe_model` response enriched with column-group tags.
///
/// Matches `enriched-catalog.v1` schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnrichedCatalog {
    /// The model name / path (e.g. `postgres.tpcds.tpcds_benchmark_model`).
    pub model: String,
    /// All columns in the model, enriched with `column_group` sets.
    pub columns: Vec<EnrichedColumn>,
}

// ---------------------------------------------------------------------------
// CompatibilityMatrix
// ---------------------------------------------------------------------------

/// Serializable output of [`build_matrix`].
///
/// Emitted under the `compatibility-matrix.v1` key in `describe_model` responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompatibilityMatrix {
    /// Schema version key.
    pub schema: String,
    /// For each measure `unique_name`, the set of compatible hierarchy ids.
    ///
    /// A hierarchy is compatible with a measure iff their column-group sets intersect.
    pub measures: BTreeMap<String, MeasureCompatibility>,
    /// For each hierarchy id, the set of compatible measure `unique_name`s.
    ///
    /// This is the inverse of `measures` and is reconstructable from it.
    /// When [`MatrixConfig::include_inverse`] is `false` (payload budget exceeded),
    /// this map is empty.
    pub hierarchies: BTreeMap<String, HierarchyCompatibility>,
    /// Diagnostic note, present when the catalog had no `column_group` data
    /// or when the inverse index was dropped to meet the payload budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Compatible hierarchy ids for one measure.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MeasureCompatibility {
    /// Hierarchy ids compatible with this measure.
    pub compatible_hierarchies: BTreeSet<String>,
}

/// Compatible measure `unique_name`s for one hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HierarchyCompatibility {
    /// Measure `unique_name`s compatible with this hierarchy.
    pub compatible_measures: BTreeSet<String>,
}

/// Configuration for [`build_matrix`].
#[derive(Debug, Clone)]
pub struct MatrixConfig {
    /// Approximate character budget for the serialized matrix JSON.
    ///
    /// When the forward-map serialization alone would exceed this budget,
    /// `include_inverse` is forced to `false`. Set to `usize::MAX` to disable.
    pub payload_budget_chars: usize,
    /// Whether to include the inverse index (`hierarchies` map).
    ///
    /// When `false`, the `hierarchies` map is emitted as empty and a note
    /// is added explaining how to reconstruct it from `measures`.
    pub include_inverse: bool,
}

impl Default for MatrixConfig {
    fn default() -> Self {
        Self {
            payload_budget_chars: 1_000_000,
            include_inverse: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Core algorithm
// ---------------------------------------------------------------------------

/// Compute the compatibility matrix from an enriched catalog.
///
/// # Algorithm
///
/// 1. Collect all measures (columns where `kind == "measure"`).
/// 2. Collect all dimension levels grouped by `hierarchy` id.
/// 3. For each hierarchy, compute the union of its levels' `column_group` sets.
/// 4. For each (measure, hierarchy) pair, check if the measure's `column_group`
///    set intersects the hierarchy's union set → compatible.
/// 5. Build the symmetric forward + inverse index.
/// 6. If `include_inverse && payload over budget`, drop inverse and add note.
///
/// # Fail-safe
///
/// When no `column_group` data is present (all sets empty), returns an empty
/// matrix with a `note` field — never an all-compatible matrix.
///
/// # Examples
///
/// ```
/// use mqoguard_compatibility_matrix::{EnrichedCatalog, EnrichedColumn, MatrixConfig, build_matrix};
/// use std::collections::BTreeSet;
///
/// let catalog = EnrichedCatalog {
///     model: "test".into(),
///     columns: vec![
///         EnrichedColumn {
///             unique_name: "sales_amount".into(),
///             label: "Sales Amount".into(),
///             kind: "measure".into(),
///             is_calc: false,
///             hierarchy: None,
///             level: None,
///             column_group: BTreeSet::from(["sales".into()]),
///         },
///         EnrichedColumn {
///             unique_name: "date_day".into(),
///             label: "Date Day".into(),
///             kind: "dimension".into(),
///             is_calc: false,
///             hierarchy: Some("Date".into()),
///             level: Some("Day".into()),
///             column_group: BTreeSet::from(["sales".into()]),
///         },
///     ],
/// };
/// let matrix = build_matrix(&catalog, &MatrixConfig::default());
/// let compat = &matrix.measures["sales_amount"].compatible_hierarchies;
/// assert!(compat.contains("Date"));
/// ```
#[must_use]
pub fn build_matrix(catalog: &EnrichedCatalog, config: &MatrixConfig) -> CompatibilityMatrix {
    let measures: Vec<&EnrichedColumn> = catalog
        .columns
        .iter()
        .filter(|c| c.kind == "measure")
        .collect();
    let hierarchy_groups = collect_hierarchy_groups(&catalog.columns);

    // Fail-safe: if no column_group data is present, return empty matrix with note.
    let any_measure_groups = measures.iter().any(|m| !m.column_group.is_empty());
    let any_hier_groups = hierarchy_groups.values().any(|s| !s.is_empty());
    if !any_measure_groups || !any_hier_groups {
        return empty_matrix_with_note();
    }

    let measure_compat = compute_forward_map(&measures, &hierarchy_groups);
    let hier_compat = compute_inverse_map(&measure_compat, &hierarchy_groups);

    debug_assert!(
        verify_symmetry(&measure_compat, &hier_compat),
        "internal: compatibility matrix is not symmetric — this is a bug"
    );

    emit_matrix(measure_compat, hier_compat, config)
}

/// Collect dimension levels grouped by hierarchy id.
///
/// Maps hierarchy id → union of `column_group` sets across all its levels.
fn collect_hierarchy_groups(
    columns: &[EnrichedColumn],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for col in columns {
        if col.kind != "measure" {
            if let Some(hier) = &col.hierarchy {
                groups
                    .entry(hier.clone())
                    .or_default()
                    .extend(col.column_group.iter().cloned());
            }
        }
    }
    groups
}

/// Return an empty [`CompatibilityMatrix`] with the no-column-group diagnostic note.
fn empty_matrix_with_note() -> CompatibilityMatrix {
    CompatibilityMatrix {
        schema: "compatibility-matrix.v1".into(),
        measures: BTreeMap::new(),
        hierarchies: BTreeMap::new(),
        note: Some(
            "No column_group data present in the enriched catalog. \
             Compatibility cannot be determined. Run mqoguard-column-group-enrichment \
             to tag the catalog before computing the matrix."
                .into(),
        ),
    }
}

/// Compute the forward map: measure `unique_name` → set of compatible hierarchy ids.
fn compute_forward_map(
    measures: &[&EnrichedColumn],
    hierarchy_groups: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeMap<String, BTreeSet<String>> {
    measures
        .iter()
        .map(|m| {
            let compatible = hierarchy_groups
                .iter()
                .filter(|(_, hg)| !m.column_group.is_disjoint(hg))
                .map(|(id, _)| id.clone())
                .collect();
            (m.unique_name.clone(), compatible)
        })
        .collect()
}

/// Compute the inverse map: hierarchy id → set of compatible measure `unique_name`s.
///
/// Pre-populates every hierarchy (including those with 0 compatible measures)
/// so the inverse index is complete.
fn compute_inverse_map(
    measure_compat: &BTreeMap<String, BTreeSet<String>>,
    hierarchy_groups: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut inv: BTreeMap<String, BTreeSet<String>> = hierarchy_groups
        .keys()
        .map(|k| (k.clone(), BTreeSet::new()))
        .collect();
    for (measure_name, hier_set) in measure_compat {
        for hier_id in hier_set {
            inv.entry(hier_id.clone()).or_default().insert(measure_name.clone());
        }
    }
    inv
}

/// Wrap the raw compat maps into a [`CompatibilityMatrix`], applying budget policy.
fn emit_matrix(
    measure_compat: BTreeMap<String, BTreeSet<String>>,
    hier_compat: BTreeMap<String, BTreeSet<String>>,
    config: &MatrixConfig,
) -> CompatibilityMatrix {
    let forward_map: BTreeMap<String, MeasureCompatibility> = measure_compat
        .into_iter()
        .map(|(k, v)| (k, MeasureCompatibility { compatible_hierarchies: v }))
        .collect();

    let forward_json = serde_json::to_string(&forward_map).unwrap_or_default();
    let within_budget = forward_json.len() <= config.payload_budget_chars;
    let include_inverse = config.include_inverse && within_budget;

    let (hierarchies, note) = if include_inverse {
        let inv = hier_compat
            .into_iter()
            .map(|(k, v)| (k, HierarchyCompatibility { compatible_measures: v }))
            .collect();
        (inv, None)
    } else {
        let note = "Inverse index (hierarchy->measures) omitted to meet payload budget. \
                    Reconstruct by inverting the measures map: for each measure M and each \
                    hierarchy H in M.compatible_hierarchies, add M to H.compatible_measures."
            .to_string();
        (BTreeMap::new(), Some(note))
    };

    CompatibilityMatrix {
        schema: "compatibility-matrix.v1".into(),
        measures: forward_map,
        hierarchies,
        note,
    }
}

/// Verify symmetry: measure M lists hierarchy H iff H lists measure M.
///
/// Returns `true` iff the relation is symmetric.
fn verify_symmetry(
    measure_compat: &BTreeMap<String, BTreeSet<String>>,
    hier_compat: &BTreeMap<String, BTreeSet<String>>,
) -> bool {
    // Forward check: for each (M, H) in measure_compat, H must list M
    for (measure, hiers) in measure_compat {
        for hier in hiers {
            let Some(measures_for_hier) = hier_compat.get(hier) else {
                return false;
            };
            if !measures_for_hier.contains(measure) {
                return false;
            }
        }
    }
    // Reverse check: for each (H, M) in hier_compat, M must list H
    for (hier, measures) in hier_compat {
        for measure in measures {
            let Some(hiers_for_measure) = measure_compat.get(measure) else {
                return false;
            };
            if !hiers_for_measure.contains(hier) {
                return false;
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Public symmetry verification (exposed for consumers and tests)
// ---------------------------------------------------------------------------

/// Verify that a [`CompatibilityMatrix`] is symmetric.
///
/// Returns `true` iff for every (measure M, hierarchy H) pair:
/// M's compatible set contains H iff H's compatible set contains M.
///
/// When the inverse index was dropped (empty `hierarchies` map), this
/// function rebuilds the inverse from the forward map and verifies against it.
#[must_use]
pub fn is_symmetric(matrix: &CompatibilityMatrix) -> bool {
    // Build effective inverse from the forward map (handles dropped-index case)
    let mut effective_inverse: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (measure, mc) in &matrix.measures {
        for hier in &mc.compatible_hierarchies {
            effective_inverse
                .entry(hier.as_str())
                .or_default()
                .insert(measure.as_str());
        }
    }

    // If the stored hierarchies map is non-empty, check it matches the effective inverse
    if !matrix.hierarchies.is_empty() {
        for (hier, hc) in &matrix.hierarchies {
            let expected = effective_inverse.get(hier.as_str()).cloned().unwrap_or_default();
            let actual: BTreeSet<&str> = hc.compatible_measures.iter().map(String::as_str).collect();
            if expected != actual {
                return false;
            }
        }
        // Also check every entry in effective_inverse exists in the stored map
        for (hier, expected) in &effective_inverse {
            let stored = matrix
                .hierarchies
                .get(*hier)
                .map_or_else(BTreeSet::new, |hc| {
                    hc.compatible_measures.iter().map(String::as_str).collect::<BTreeSet<_>>()
                });
            if expected != &stored {
                return false;
            }
        }
    }

    true
}

// ---------------------------------------------------------------------------
// JSON emission helper
// ---------------------------------------------------------------------------

/// Serialize a [`CompatibilityMatrix`] to a JSON value keyed under
/// `compatibility-matrix.v1`, ready to embed in a `describe_model` response.
///
/// # Errors
///
/// Returns a `serde_json::Error` if serialization fails (should not occur
/// for well-formed data).
pub fn to_describe_model_fragment(
    matrix: &CompatibilityMatrix,
) -> Result<serde_json::Value, serde_json::Error> {
    let inner = serde_json::to_value(matrix)?;
    Ok(serde_json::json!({ "compatibility-matrix.v1": inner }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn measure(unique_name: &str, groups: &[&str]) -> EnrichedColumn {
        EnrichedColumn {
            unique_name: unique_name.into(),
            label: unique_name.into(),
            kind: "measure".into(),
            is_calc: false,
            hierarchy: None,
            level: None,
            column_group: groups.iter().map(|&s| s.to_string()).collect(),
        }
    }

    fn level(unique_name: &str, hierarchy: &str, groups: &[&str]) -> EnrichedColumn {
        EnrichedColumn {
            unique_name: unique_name.into(),
            label: unique_name.into(),
            kind: "dimension".into(),
            is_calc: false,
            hierarchy: Some(hierarchy.into()),
            level: Some(unique_name.into()),
            column_group: groups.iter().map(|&s| s.to_string()).collect(),
        }
    }

    fn catalog(columns: Vec<EnrichedColumn>) -> EnrichedCatalog {
        EnrichedCatalog { model: "test_model".into(), columns }
    }

    #[test]
    fn incompatible_pair_excluded() {
        // Measure in {sales}, hierarchy levels all in {inventory} → incompatible
        let cat = catalog(vec![
            measure("sales_amount", &["sales"]),
            level("inv_level_1", "InventoryHier", &["inventory"]),
        ]);
        let m = build_matrix(&cat, &MatrixConfig::default());
        let compat = m.measures.get("sales_amount").is_some_and(|mc| mc.compatible_hierarchies.contains("InventoryHier"));
        assert!(
            !compat,
            "sales_amount should NOT be compatible with InventoryHier"
        );
    }

    #[test]
    fn compatible_pair_included() {
        // Measure in {sales}, hierarchy levels in {sales} → compatible
        let cat = catalog(vec![
            measure("sales_amount", &["sales"]),
            level("date_day", "DateHier", &["sales"]),
        ]);
        let m = build_matrix(&cat, &MatrixConfig::default());
        let compat = m.measures.get("sales_amount").is_some_and(|mc| mc.compatible_hierarchies.contains("DateHier"));
        assert!(
            compat,
            "sales_amount should be compatible with DateHier"
        );
    }

    #[test]
    fn symmetry_holds() {
        let cat = catalog(vec![
            measure("sales_amount", &["sales"]),
            measure("inv_qty", &["inventory"]),
            level("date_day", "DateHier", &["sales"]),
            level("inv_date", "InvDateHier", &["inventory"]),
            level("product_name", "ProductHier", &["sales", "inventory"]),
        ]);
        let m = build_matrix(&cat, &MatrixConfig::default());
        assert!(is_symmetric(&m), "matrix must be symmetric");
    }

    #[test]
    fn empty_groups_returns_empty_matrix_with_note() {
        let cat = catalog(vec![
            measure("sales_amount", &[]),
            level("date_day", "DateHier", &[]),
        ]);
        let m = build_matrix(&cat, &MatrixConfig::default());
        assert!(m.measures.is_empty(), "no column_groups → empty measures map");
        assert!(m.note.is_some(), "no column_groups → note present");
        let note = m.note.as_ref().map_or("", String::as_str);
        assert!(
            !note.is_empty(),
            "note should explain missing column_group data"
        );
    }

    #[test]
    fn budget_exceeded_drops_inverse() {
        let cat = catalog(vec![
            measure("sales_amount", &["sales"]),
            level("date_day", "DateHier", &["sales"]),
        ]);
        let config = MatrixConfig {
            payload_budget_chars: 1, // tiny budget forces drop
            include_inverse: true,
        };
        let m = build_matrix(&cat, &config);
        assert!(m.hierarchies.is_empty(), "inverse index should be dropped");
        assert!(m.note.is_some(), "note should explain dropped index");
    }

    #[test]
    fn fragment_is_keyed() {
        let cat = catalog(vec![
            measure("sales_amount", &["sales"]),
            level("date_day", "DateHier", &["sales"]),
        ]);
        let m = build_matrix(&cat, &MatrixConfig::default());
        let frag = to_describe_model_fragment(&m);
        assert!(frag.is_ok(), "serialization must succeed");
        if let Ok(f) = frag {
            assert!(
                f.get("compatibility-matrix.v1").is_some(),
                "fragment must be keyed under compatibility-matrix.v1"
            );
        }
    }
}
