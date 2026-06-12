//! Core binding logic: resolve MQO references against a `CatalogSnapshot`.

use crate::catalog::{CatalogSnapshot, ColumnEntry};
use crate::compat::EnrichedColumnGroups;
use mqo_spec::{Filter, LevelSelection, Mqo};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Output types ──────────────────────────────────────────────────────────────

/// Extended `BoundMeasure` with semi-additive trigger hierarchies.
/// We extend beyond the `mqo_spec::BoundMeasure` to carry R7/R11 metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundMeasureExt {
    pub unique_name: String,
    pub is_calc: bool,
    pub semi_additive: bool,
    pub trigger_hierarchies: Vec<String>,
    pub required_dimension: Option<String>,
}

/// Extended `BoundDimension`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundDimensionExt {
    pub unique_name: String,
    pub hierarchy: String,
}

/// A resolved calc-group member, carrying its MDX.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundCalcGroupMember {
    pub calc_group: String,
    pub member: String,
    pub unique_name: String,
    pub mdx: String,
}

/// The successfully-bound output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundMqoOutput {
    /// Original MQO echoed back.
    pub mqo: Mqo,
    /// Resolved measure bindings.
    pub measures: Vec<BoundMeasureExt>,
    /// Resolved dimension bindings.
    pub dimensions: Vec<BoundDimensionExt>,
    /// Resolved calc-group member bindings (from `CalcGroupMember` filters).
    pub calc_group_members: Vec<BoundCalcGroupMember>,
}

/// One incompatible measure×dimension pair: column-group sets are disjoint and neither is conformed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncompatibilityReport {
    pub measure_unique_name: String,
    pub dimension_unique_name: String,
    /// Sorted list of column-group identifiers for the measure.
    pub measure_column_groups: Vec<String>,
    /// Sorted list of column-group identifiers for the dimension.
    pub dimension_column_groups: Vec<String>,
    /// Human-readable explanation. Stable format for log lines.
    pub note: String,
}

/// The result of `bind()` / `bind_with_compat()`.
#[derive(Debug)]
pub enum BindResult {
    Bound(Box<BoundMqoOutput>),
    /// One or more refs matched multiple `unique_names` (case-insensitive label collision).
    Ambiguous(Vec<Value>),
    /// One or more refs matched nothing.
    NotFound(Vec<String>),
    /// One or more measure×dimension pairs span different facts (only from `bind_with_compat`).
    Incompatible(Vec<IncompatibilityReport>),
}

// ── Resolution helpers ────────────────────────────────────────────────────────

/// Resolve a label (or `unique_name`) against the column list for measures.
///
/// Returns:
///   `Ok(entry)` — exactly one match
///   `Err(candidates)` — zero (empty vec) or multiple matches
fn resolve_measure<'a>(
    label: &str,
    columns: &'a [ColumnEntry],
) -> Result<&'a ColumnEntry, Vec<&'a ColumnEntry>> {
    let key = label.to_lowercase();

    // First: try exact unique_name match (case-insensitive)
    let by_unique: Vec<&ColumnEntry> = columns
        .iter()
        .filter(|c| c.kind == "measure" && c.unique_name.to_lowercase() == key)
        .collect();

    if by_unique.len() == 1 {
        return Ok(by_unique[0]);
    }
    if by_unique.len() > 1 {
        return Err(by_unique);
    }

    // Second: try label match (case-insensitive)
    let by_label: Vec<&ColumnEntry> = columns
        .iter()
        .filter(|c| c.kind == "measure" && c.label.to_lowercase() == key)
        .collect();

    match by_label.len() {
        1 => Ok(by_label[0]),
        0 => Err(vec![]),
        _ => Err(by_label),
    }
}

/// Resolve a dimension level selection against the column list.
/// Matches by hierarchy (exact, case-insensitive) + level name (case-insensitive).
fn resolve_level<'a>(
    sel: &LevelSelection,
    columns: &'a [ColumnEntry],
) -> Result<&'a ColumnEntry, Vec<&'a ColumnEntry>> {
    let hier_key = sel.hierarchy.to_lowercase();
    let level_key = sel.level.to_lowercase();

    let candidates: Vec<&ColumnEntry> = columns
        .iter()
        .filter(|c| {
            c.kind == "level"
                && c.hierarchy
                    .as_deref()
                    .is_some_and(|h| h.to_lowercase() == hier_key)
                && c.level
                    .as_deref()
                    .is_some_and(|l| l.to_lowercase() == level_key)
        })
        .collect();

    match candidates.len() {
        1 => Ok(candidates[0]),
        0 => Err(vec![]),
        _ => Err(candidates),
    }
}

// ── Main bind function ────────────────────────────────────────────────────────

/// Bind an MQO against a catalog snapshot.
///
/// Resolution precedence within each category:
/// 1. Exact `unique_name` match (case-insensitive)
/// 2. `label` match (case-insensitive)
///
/// If any ref is ambiguous, returns `BindResult::Ambiguous`.
/// If any ref is missing (and none are ambiguous), returns `BindResult::NotFound`.
/// Only if all refs resolve returns `BindResult::Bound`.
///
/// Priority: ambiguous > `not_found` > bound (i.e., if both exist, ambiguous wins).
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn bind(mqo: &Mqo, snapshot: &CatalogSnapshot) -> BindResult {
    let mut ambiguous: Vec<Value> = vec![];
    let mut not_found: Vec<String> = vec![];

    // ── Resolve measures ──────────────────────────────────────────────────
    let mut bound_measures: Vec<BoundMeasureExt> = vec![];

    for m_ref in &mqo.measures {
        match resolve_measure(&m_ref.unique_name, &snapshot.columns) {
            Ok(entry) => {
                let (semi_additive, trigger_hierarchies) = entry
                    .semi_additive
                    .as_ref()
                    .map_or((false, vec![]), |sa| (true, sa.trigger_hierarchies.clone()));

                bound_measures.push(BoundMeasureExt {
                    unique_name: entry.unique_name.clone(),
                    is_calc: entry.is_calc,
                    semi_additive,
                    trigger_hierarchies,
                    required_dimension: entry.required_dimension.clone(),
                });
            }
            Err(candidates) if candidates.is_empty() => {
                not_found.push(m_ref.unique_name.clone());
            }
            Err(candidates) => {
                ambiguous.push(serde_json::json!({
                    "ref": m_ref.unique_name,
                    "candidates": candidates.iter().map(|c| &c.unique_name).collect::<Vec<_>>(),
                }));
            }
        }
    }

    // ── Resolve dimensions ────────────────────────────────────────────────
    let mut bound_dimensions: Vec<BoundDimensionExt> = vec![];

    for sel in &mqo.dimensions {
        match resolve_level(sel, &snapshot.columns) {
            Ok(entry) => {
                bound_dimensions.push(BoundDimensionExt {
                    unique_name: entry.unique_name.clone(),
                    hierarchy: entry
                        .hierarchy
                        .clone()
                        .unwrap_or_else(|| sel.hierarchy.clone()),
                });
            }
            Err(candidates) if candidates.is_empty() => {
                not_found.push(format!("dimension {}:{}", sel.hierarchy, sel.level));
            }
            Err(candidates) => {
                ambiguous.push(serde_json::json!({
                    "ref": format!("dimension {}:{}", sel.hierarchy, sel.level),
                    "candidates": candidates.iter().map(|c| &c.unique_name).collect::<Vec<_>>(),
                }));
            }
        }
    }

    // ── Resolve CalcGroupMember filters ───────────────────────────────────
    let mut bound_calc_group_members: Vec<BoundCalcGroupMember> = vec![];

    for filter in &mqo.filters {
        if let Filter::CalcGroupMember { calc_group, member } = filter {
            let group_key = calc_group.to_lowercase();
            let member_key = member.to_lowercase();

            let calc_entries = snapshot
                .describe_model
                .as_ref()
                .map_or(&[][..], |dm| dm.calc_groups.as_slice());

            let matches: Vec<_> = calc_entries
                .iter()
                .filter(|e| {
                    e.group_name.to_lowercase() == group_key
                        && e.member_name.to_lowercase() == member_key
                })
                .collect();

            match matches.len() {
                1 => {
                    let e = matches[0];
                    bound_calc_group_members.push(BoundCalcGroupMember {
                        calc_group: calc_group.clone(),
                        member: member.clone(),
                        unique_name: e.unique_name.clone(),
                        mdx: e.mdx.clone(),
                    });
                }
                0 => {
                    not_found.push(format!("calc_group_member {calc_group}::{member}"));
                }
                _ => {
                    ambiguous.push(serde_json::json!({
                        "ref": format!("calc_group_member {calc_group}::{member}"),
                        "candidates": matches.iter().map(|e| &e.unique_name).collect::<Vec<_>>(),
                    }));
                }
            }
        }
    }

    // ── Collate results ───────────────────────────────────────────────────
    if !ambiguous.is_empty() {
        return BindResult::Ambiguous(ambiguous);
    }
    if !not_found.is_empty() {
        return BindResult::NotFound(not_found);
    }

    BindResult::Bound(Box::new(BoundMqoOutput {
        mqo: mqo.clone(),
        measures: bound_measures,
        dimensions: bound_dimensions,
        calc_group_members: bound_calc_group_members,
    }))
}

/// Bind an MQO against a catalog snapshot, then run a cross-fact compatibility check.
#[must_use]
///
/// Requires an `enriched-catalog.v1` group map from `EnrichedColumnGroups::from_path`.
/// Precedence: `NotFound` / `Ambiguous` > `Incompatible` > `Bound`.
/// When the enriched catalog is absent, call `bind()` directly (FR7).
pub fn bind_with_compat(
    mqo: &Mqo,
    snapshot: &CatalogSnapshot,
    enriched: &EnrichedColumnGroups,
) -> BindResult {
    match bind(mqo, snapshot) {
        BindResult::Bound(bound) => {
            let reports = check_cross_fact_paths(&bound, enriched);
            if reports.is_empty() {
                BindResult::Bound(bound)
            } else {
                BindResult::Incompatible(reports)
            }
        }
        other => other,
    }
}

fn check_cross_fact_paths(
    bound: &BoundMqoOutput,
    enriched: &EnrichedColumnGroups,
) -> Vec<IncompatibilityReport> {
    let mut reports = Vec::new();

    for measure in &bound.measures {
        let m_groups = enriched.groups_for(&measure.unique_name);
        if EnrichedColumnGroups::is_conformed(m_groups) {
            continue;
        }
        for dimension in &bound.dimensions {
            let d_groups = enriched.groups_for(&dimension.unique_name);
            if EnrichedColumnGroups::is_conformed(d_groups) {
                continue;
            }
            let intersects = m_groups.iter().any(|g| d_groups.contains(g));
            if !intersects {
                // BTreeSet iterates in sorted order — Vec is already sorted.
                let m_vec: Vec<String> = m_groups.iter().cloned().collect();
                let d_vec: Vec<String> = d_groups.iter().cloned().collect();
                reports.push(IncompatibilityReport {
                    measure_unique_name: measure.unique_name.clone(),
                    dimension_unique_name: dimension.unique_name.clone(),
                    note: format!(
                        "measure `{}` (groups: {}) and dimension `{}` (groups: {}) share no fact",
                        measure.unique_name,
                        m_vec.join(", "),
                        dimension.unique_name,
                        d_vec.join(", ")
                    ),
                    measure_column_groups: m_vec,
                    dimension_column_groups: d_vec,
                });
            }
        }
    }

    // Deterministic order: measure unique_name, then dimension unique_name.
    reports.sort_by(|a, b| {
        a.measure_unique_name
            .cmp(&b.measure_unique_name)
            .then_with(|| a.dimension_unique_name.cmp(&b.dimension_unique_name))
    });

    reports
}

#[cfg(test)]
#[allow(clippy::doc_markdown)]
mod binder_unit_tests {
    use super::*;
    use crate::catalog::{CalcGroupEntry, ColumnEntry, DescribeModelOutput, SemiAdditiveInfo};
    use mqo_spec::{Filter, LevelSelection, MeasureRef, Mqo};

    fn make_measure(unique_name: &str, label: &str) -> ColumnEntry {
        ColumnEntry {
            unique_name: unique_name.to_string(),
            label: label.to_string(),
            kind: "measure".to_string(),
            hierarchy: None,
            level: None,
            semi_additive: None,
            required_dimension: None,
            is_calc: false,
        }
    }

    fn make_level(unique_name: &str, label: &str, hierarchy: &str, level: &str) -> ColumnEntry {
        ColumnEntry {
            unique_name: unique_name.to_string(),
            label: label.to_string(),
            kind: "level".to_string(),
            hierarchy: Some(hierarchy.to_string()),
            level: Some(level.to_string()),
            semi_additive: None,
            required_dimension: None,
            is_calc: false,
        }
    }

    #[test]
    fn resolve_measure_exact_unique_name() {
        let cols = vec![make_measure("sales.revenue", "Revenue")];
        let result = resolve_measure("sales.revenue", &cols);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().unique_name, "sales.revenue");
    }

    #[test]
    fn resolve_measure_by_label_case_insensitive() {
        let cols = vec![make_measure("sales.revenue", "Revenue")];
        let result = resolve_measure("REVENUE", &cols);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().unique_name, "sales.revenue");
    }

    #[test]
    fn resolve_measure_not_found_returns_empty_err() {
        let cols = vec![make_measure("sales.revenue", "Revenue")];
        let result = resolve_measure("NonExistent", &cols);
        assert!(result.is_err());
        assert!(result.unwrap_err().is_empty());
    }

    #[test]
    fn resolve_measure_ambiguous_returns_candidates() {
        let cols = vec![
            make_measure("model_a.revenue", "Revenue"),
            make_measure("model_b.revenue", "Revenue"),
        ];
        let result = resolve_measure("Revenue", &cols);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().len(), 2);
    }

    #[test]
    fn resolve_level_case_insensitive() {
        let cols = vec![make_level(
            "time.calendar.[Year]",
            "Year",
            "time.calendar",
            "Year",
        )];
        let sel = LevelSelection {
            hierarchy: "time.calendar".to_string(),
            level: "year".to_string(),
        };
        let result = resolve_level(&sel, &cols);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().unique_name, "time.calendar.[Year]");
    }

    #[test]
    fn bind_semi_additive_flags() {
        let columns = vec![ColumnEntry {
            unique_name: "sales.balance".to_string(),
            label: "Balance".to_string(),
            kind: "measure".to_string(),
            hierarchy: None,
            level: None,
            semi_additive: Some(SemiAdditiveInfo {
                trigger_hierarchies: vec!["time.calendar".to_string()],
            }),
            required_dimension: Some("account.type".to_string()),
            is_calc: false,
        }];
        let snapshot = CatalogSnapshot {
            columns,
            ..CatalogSnapshot::default()
        };
        let mqo = Mqo {
            model: "sales".to_string(),
            measures: vec![MeasureRef {
                unique_name: "Balance".to_string(),
            }],
            dimensions: vec![],
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
        };
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::Bound(b) => {
                assert!(b.measures[0].semi_additive);
                assert_eq!(b.measures[0].trigger_hierarchies, vec!["time.calendar"]);
                assert_eq!(
                    b.measures[0].required_dimension,
                    Some("account.type".to_string())
                );
            }
            other => panic!("expected Bound, got {other:?}"),
        }
    }

    #[test]
    fn bind_calc_group_member_missing_describe_model() {
        let snapshot = CatalogSnapshot {
            columns: vec![make_measure("sales.revenue", "Revenue")],
            ..CatalogSnapshot::default()
        };
        let mqo = Mqo {
            model: "sales".to_string(),
            measures: vec![MeasureRef {
                unique_name: "Revenue".to_string(),
            }],
            dimensions: vec![],
            filters: vec![Filter::CalcGroupMember {
                calc_group: "Time Intelligence".to_string(),
                member: "YTD".to_string(),
            }],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
        };
        let result = bind(&mqo, &snapshot);
        // No describe_model → member not found
        assert!(matches!(result, BindResult::NotFound(_)));
    }

    // ── Missed-mutant killers (cargo-mutants iter-2) ──────────────────────────

    /// Kills mutant: `by_unique.len() > 1` → `by_unique.len() == 1` (line 82).
    /// When there are TWO entries with the same unique_name (case-insensitive),
    /// resolution must return Err (ambiguous), not Ok.
    #[test]
    fn resolve_measure_duplicate_unique_name_is_ambiguous() {
        let cols = vec![
            // Two different entries resolving to the same lowercase unique_name:
            // this can happen if the catalog has e.g. "Sales.Revenue" and "sales.revenue".
            make_measure("sales.REVENUE", "Revenue Dup A"),
            make_measure("sales.revenue", "Revenue Dup B"),
        ];
        // "sales.revenue" matches both (case-insensitive).
        let result = resolve_measure("sales.revenue", &cols);
        // Must be Err with 2 candidates — NOT Ok.
        assert!(result.is_err(), "duplicate unique_name must be Err, not Ok");
        let candidates = result.unwrap_err();
        assert_eq!(candidates.len(), 2, "must report both candidates");
    }

    /// Kills mutant: `by_unique.len() > 1` → `by_unique.len() >= 1` (line 82).
    /// With `>= 1`, a single unique_name match would fall to Err instead of Ok.
    /// This test asserts exact unique_name match with one match → Ok (not Err).
    /// To maximally distinguish from `>= 1`: the catalog also has a same-label
    /// entry so that if the unique_name path fails, the label-path would find TWO
    /// entries (ambiguous) not one — proving the unique_name fast-path fired.
    #[test]
    fn resolve_measure_single_unique_name_match_preempts_label_search() {
        let cols = vec![
            // Exact unique_name match for "sales.units"
            make_measure("sales.units", "Units"),
            // Another measure that has the same label "Units" — label path would be ambiguous
            make_measure("other.units", "Units"),
        ];
        // Searching by unique_name: exactly one entry matches → must be Ok
        let result = resolve_measure("sales.units", &cols);
        assert!(
            result.is_ok(),
            "single unique_name match must return Ok, got: {result:?}"
        );
        assert_eq!(result.unwrap().unique_name, "sales.units");
        // (If the >= mutant fired, it would fall through to label search and find
        // two "Units" entries → Err with 2 candidates, which the assert above catches.)
    }

    /// Kills mutant: `0 =>` arm deleted in resolve_measure (line 94).
    /// When the label is genuinely not in the catalog, the result must be
    /// Err with an empty candidates vec, not Ok or a non-empty Err.
    #[test]
    fn resolve_measure_not_found_gives_empty_vec() {
        let cols = vec![make_measure("sales.revenue", "Revenue")];
        let result = resolve_measure("TotallyAbsent", &cols);
        assert!(result.is_err(), "not-found must be Err");
        // The Err vec must be empty (distinguishes not-found from ambiguous).
        assert!(
            result.unwrap_err().is_empty(),
            "not-found Err vec must be empty"
        );
    }

    /// Kills mutant: `0 =>` arm deleted in resolve_level (line 123).
    /// An unknown level must produce Err with an empty vec, not Ok.
    #[test]
    fn resolve_level_not_found_gives_empty_vec() {
        let cols = vec![make_level(
            "time.calendar.[Year]",
            "Year",
            "time.calendar",
            "Year",
        )];
        let sel = LevelSelection {
            hierarchy: "time.calendar".to_string(),
            level: "Decade".to_string(), // does not exist
        };
        let result = resolve_level(&sel, &cols);
        assert!(result.is_err(), "not-found level must be Err");
        assert!(
            result.unwrap_err().is_empty(),
            "not-found level Err vec must be empty"
        );
    }

    /// Kills mutant: match guard `candidates.is_empty()` → `true` in bind (line 191).
    /// An ambiguous dimension (two levels with the same name) must surface as
    /// BindResult::Ambiguous, not BindResult::NotFound.
    #[test]
    fn bind_ambiguous_dimension_is_ambiguous_not_not_found() {
        let cols = vec![
            // Two levels with the same hierarchy+level combo (shouldn't happen in
            // a well-formed catalog, but the binder must handle it correctly).
            make_level("time.calendar.[Year]",  "Year", "time.calendar", "Year"),
            make_level("time.fiscal.[Year]",    "Year", "time.calendar", "Year"),
        ];
        let snapshot = CatalogSnapshot { columns: cols, ..CatalogSnapshot::default() };
        let mqo = Mqo {
            model: "sales".to_string(),
            measures: vec![MeasureRef { unique_name: "NonExistentMeasureXYZ".to_string() }],
            dimensions: vec![LevelSelection {
                hierarchy: "time.calendar".to_string(),
                level: "Year".to_string(),
            }],
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
        };
        let result = bind(&mqo, &snapshot);
        // Dimension is ambiguous (2 candidates) — must surface as Ambiguous.
        // (Ambiguous takes precedence even though measure is also not-found.)
        assert!(
            matches!(result, BindResult::Ambiguous(_)),
            "ambiguous dimension must yield BindResult::Ambiguous, got: {result:?}"
        );
    }

    /// Reviewer counter-attack: a level ColumnEntry with hierarchy=None is invisible
    /// to dimension binding (because resolve_level requires both hierarchy and level to
    /// be Some). An MQO requesting that level gets not_found. Documents the behavior
    /// so future changes can't silently change it.
    #[test]
    fn level_entry_with_missing_hierarchy_is_invisible_to_binding() {
        let cols = vec![ColumnEntry {
            unique_name: "time.year".to_string(),
            label: "Year".to_string(),
            kind: "level".to_string(),
            hierarchy: None, // malformed catalog entry
            level: None,     // malformed catalog entry
            semi_additive: None,
            required_dimension: None,
            is_calc: false,
        }];
        let snapshot = CatalogSnapshot {
            columns: cols,
            ..CatalogSnapshot::default()
        };
        let mqo = Mqo {
            model: "m".to_string(),
            measures: vec![],
            dimensions: vec![LevelSelection {
                hierarchy: "time".to_string(),
                level: "Year".to_string(),
            }],
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
        };
        // The malformed entry is invisible to resolve_level (both hierarchy and level None)
        // → not_found. Documents the documented non-goal "no catalog validation."
        assert!(
            matches!(bind(&mqo, &snapshot), BindResult::NotFound(_)),
            "level entry with missing hierarchy/level fields must produce not_found"
        );
    }

    #[test]
    fn bind_calc_group_member_found() {
        let snapshot = CatalogSnapshot {
            columns: vec![make_measure("sales.revenue", "Revenue")],
            describe_model: Some(DescribeModelOutput {
                calc_groups: vec![CalcGroupEntry {
                    group_name: "TI".to_string(),
                    member_name: "QTD".to_string(),
                    unique_name: "calc.ti.QTD".to_string(),
                    mdx: "SomeMDX()".to_string(),
                }],
            }),
            ..CatalogSnapshot::default()
        };
        let mqo = Mqo {
            model: "sales".to_string(),
            measures: vec![MeasureRef {
                unique_name: "Revenue".to_string(),
            }],
            dimensions: vec![],
            filters: vec![Filter::CalcGroupMember {
                calc_group: "TI".to_string(),
                member: "qtd".to_string(), // case-insensitive
            }],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
        };
        let result = bind(&mqo, &snapshot);
        match result {
            BindResult::Bound(b) => {
                assert_eq!(b.calc_group_members.len(), 1);
                assert_eq!(b.calc_group_members[0].unique_name, "calc.ti.QTD");
                assert_eq!(b.calc_group_members[0].mdx, "SomeMDX()");
            }
            other => panic!("expected Bound, got {other:?}"),
        }
    }
}
