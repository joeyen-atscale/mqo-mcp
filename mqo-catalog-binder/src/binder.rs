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
    /// Per-measure date-role binding (FR-1): the date hierarchy this measure is
    /// grouped on, resolved against the measure's fact. `None` when the MQO has
    /// no date dimension, when no enriched catalog is supplied, or when the date
    /// role is ambiguous/unresolvable for this measure's fact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_role_hierarchy: Option<String>,
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

/// A structured cross-fact date-role rejection (FR-2/FR-3).
///
/// Emitted when a multi-fact MQO names a single date level that is valid for one
/// fact's date role but NOT conformed to another referenced measure's fact —
/// e.g. an inventory measure grouped on a `sold_date_*` hierarchy. The
/// classification is purely catalog-structural (NFR-1, FR-5): no query is run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateRoleRejection {
    /// Stable rejection code for clients/scorers.
    pub code: String,
    /// Human-readable explanation. Stable format for log lines.
    pub detail: String,
    /// The measure that cannot be grouped on the requested date level.
    pub measure: String,
    /// The requested date hierarchy:level the measure was (wrongly) grouped on.
    pub requested_level: String,
    /// Date hierarchies that ARE valid for this measure's fact (from the catalog).
    pub valid_hierarchies: Vec<String>,
}

/// One unbound or ambiguous `Member` filter member: the member value did not
/// appear in the enumerated domain of any level in the hierarchy, or appeared
/// in multiple levels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberBindError {
    /// The hierarchy the filter was on.
    pub hierarchy: String,
    /// The member value that could not be bound.
    pub member: String,
    /// Levels whose domains were checked (each has an enumerated domain).
    pub candidate_levels: Vec<String>,
    /// Human-readable explanation, stable for log lines.
    pub note: String,
}

/// The result of `bind()` / `bind_with_compat()` / `bind_with_date_roles()`.
#[derive(Debug)]
pub enum BindResult {
    Bound(Box<BoundMqoOutput>),
    /// One or more refs matched multiple `unique_names` (case-insensitive label collision).
    Ambiguous(Vec<Value>),
    /// One or more refs matched nothing.
    NotFound(Vec<String>),
    /// One or more measure×dimension pairs span different facts (only from `bind_with_compat`).
    Incompatible(Vec<IncompatibilityReport>),
    /// A multi-fact MQO requests a date level not conformed across the referenced
    /// facts (only from `bind_with_date_roles`). Pre-execution, catalog-only.
    DateRoleIncompatible(Vec<DateRoleRejection>),
    /// One or more `Member` filter values are not in the domain of any level in
    /// the hierarchy (only when the catalog carries enumerated level domains AND
    /// all levels in the hierarchy are fully enumerated — conservative).
    MemberUnbound(Vec<MemberBindError>),
    /// One or more `Member` filter values match the domains of multiple levels
    /// in the hierarchy — caller must disambiguate.
    MemberAmbiguous(Vec<MemberBindError>),
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

// ── Member-filter domain check ────────────────────────────────────────────────

/// Attempt to resolve each `Filter::Member` value against the hierarchy's
/// enumerated level domains. Conservative: fires ONLY when ALL level columns
/// in the hierarchy carry a non-empty `domain` (if any level lacks one, a
/// high-cardinality level could hold the value → safe to skip). Returns
/// `(unbound, ambiguous)` lists; both empty means no issue found or no data
/// available.
fn check_member_filters(
    mqo: &Mqo,
    snapshot: &CatalogSnapshot,
) -> (Vec<MemberBindError>, Vec<MemberBindError>) {
    let mut unbound: Vec<MemberBindError> = Vec::new();
    let mut ambiguous: Vec<MemberBindError> = Vec::new();

    for filter in &mqo.filters {
        let Filter::Member { hierarchy, members } = filter else {
            continue;
        };
        let hier_key = hierarchy.to_lowercase();

        // All level columns in this hierarchy.
        let hier_levels: Vec<&crate::catalog::ColumnEntry> = snapshot
            .columns
            .iter()
            .filter(|c| {
                c.kind == "level"
                    && c.hierarchy
                        .as_deref()
                        .is_some_and(|h| h.to_lowercase() == hier_key)
            })
            .collect();

        if hier_levels.is_empty() {
            continue; // Hierarchy unknown — handled by the existing not_found path.
        }

        // Conservative guard: every level must have a non-empty enumerated domain.
        // Any level without one (high-cardinality, live mode) means a member could
        // legitimately belong there — skip the whole hierarchy.
        let all_enumerated = hier_levels
            .iter()
            .all(|c| c.domain.as_ref().is_some_and(|d| !d.is_empty()));
        if !all_enumerated {
            continue;
        }

        let level_names: Vec<String> = hier_levels
            .iter()
            .filter_map(|c| c.level.clone())
            .collect();

        for member in members {
            let member_norm = member.to_lowercase();

            let matching: Vec<String> = hier_levels
                .iter()
                .filter(|c| {
                    c.domain
                        .as_ref()
                        .unwrap()
                        .iter()
                        .any(|d| d.to_lowercase() == member_norm)
                })
                .filter_map(|c| c.level.clone())
                .collect();

            match matching.len() {
                0 => unbound.push(MemberBindError {
                    hierarchy: hierarchy.clone(),
                    member: member.clone(),
                    candidate_levels: level_names.clone(),
                    note: format!(
                        "member '{}' is not in the domain of any level of hierarchy '{}'; \
                         enumerated levels: {}",
                        member,
                        hierarchy,
                        level_names.join(", ")
                    ),
                }),
                1 => {} // Exactly one match — bound, no error.
                _ => ambiguous.push(MemberBindError {
                    hierarchy: hierarchy.clone(),
                    member: member.clone(),
                    candidate_levels: matching.clone(),
                    note: format!(
                        "member '{}' matches multiple levels in hierarchy '{}': {}; \
                         add a level qualifier to disambiguate",
                        member,
                        hierarchy,
                        matching.join(", ")
                    ),
                }),
            }
        }
    }

    (unbound, ambiguous)
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
                    date_role_hierarchy: None,
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

    // ── Member-filter domain check ────────────────────────────────────────
    let (member_unbound, member_ambiguous) = check_member_filters(mqo, snapshot);

    // ── Collate results ───────────────────────────────────────────────────
    // Precedence: ref-resolution errors (ambiguous/not_found) > member filter
    // errors > bound. Ref errors are authoritative; member errors only surface
    // when all refs resolved successfully.
    if !ambiguous.is_empty() {
        return BindResult::Ambiguous(ambiguous);
    }
    if !not_found.is_empty() {
        return BindResult::NotFound(not_found);
    }
    if !member_unbound.is_empty() {
        return BindResult::MemberUnbound(member_unbound);
    }
    if !member_ambiguous.is_empty() {
        return BindResult::MemberAmbiguous(member_ambiguous);
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
    // Filter-only hierarchies (Member/MemberLevel/Range filters that do not also
    // appear as projected dimensions) are inherently conformed-access: they restrict
    // rows but do not define the measure's fact context. Never flag them as cross-fact
    // incompatible (PRD-mqo-crossfact-rejection-calibration: conformed-dimension fix).
    use mqo_spec::Filter;
    let filter_hierarchies: std::collections::HashSet<&str> = bound.mqo.filters.iter()
        .filter_map(|f| match f {
            Filter::Member { hierarchy, .. } => Some(hierarchy.as_str()),
            Filter::MemberLevel { hierarchy, .. } => Some(hierarchy.as_str()),
            _ => None,
        })
        .collect();
    let projected_uniques: std::collections::HashSet<&str> =
        bound.dimensions.iter().map(|d| d.unique_name.as_str()).collect();

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
            // If this dimension is only referenced via a filter hierarchy and not
            // in the projected dimensions, treat it as conformed (calibration fix).
            let hier = dimension.unique_name
                .split('.')
                .next_back()
                .unwrap_or(&dimension.unique_name);
            if filter_hierarchies.contains(hier) && !projected_uniques.contains(dimension.unique_name.as_str()) {
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

// ── Per-measure date-role binding (FR-1) + cross-fact date rejection (FR-2/3) ──

/// Heuristic: is this a date/time hierarchy? Catalog-only, name-based.
/// TPC-DS date roles are named `*date_dimensions` / `*date_week_hierarchy`, and
/// their levels carry "Calendar"/"Date"/"Week"/"Month"/"Quarter"/"Year" labels.
fn is_date_hierarchy(hierarchy: &str) -> bool {
    let h = hierarchy.to_lowercase();
    h.contains("date") || h.contains("calendar") || h.contains("time")
}

/// Bind an MQO, then resolve a per-measure date role and reject cross-fact
/// date incompatibilities — all pre-execution, catalog-only (NFR-1).
///
/// Behaviour:
/// - Single-fact / single-date-dimension MQOs are unchanged: each measure is
///   tagged with that date hierarchy, no rejection (NFR-2, FR-4).
/// - When the MQO references measures from different facts AND date dimension(s),
///   each measure is bound to the date hierarchy whose fact intersects the
///   measure's fact (FR-1).
/// - When a measure's fact does NOT intersect ANY requested date dimension's
///   fact (the conservative incompatible case — e.g. inventory measure under a
///   `sold_date_*` level), a structured `DateRoleRejection` is emitted (FR-2/3).
/// - Fail-open: conformed entities (empty/`*` column-group) are never rejected
///   and never block binding (FR-4, FR-5).
///
/// When the MQO has no date dimension, this defers to the same blanket
/// cross-fact compatibility check as `bind_with_compat`.
///
/// Precedence: `NotFound` / `Ambiguous` (from `bind`) > `DateRoleIncompatible`
/// > `Incompatible` > `Bound`.
#[must_use]
pub fn bind_with_date_roles(
    mqo: &Mqo,
    snapshot: &CatalogSnapshot,
    enriched: &EnrichedColumnGroups,
) -> BindResult {
    let mut bound = match bind(mqo, snapshot) {
        BindResult::Bound(b) => b,
        other => return other,
    };

    // Date dimensions actually requested in this MQO, with their fact groups.
    let date_dims: Vec<(BoundDimensionExt, std::collections::BTreeSet<String>)> = bound
        .dimensions
        .iter()
        .filter(|d| is_date_hierarchy(&d.hierarchy))
        .map(|d| (d.clone(), enriched.groups_for(&d.unique_name).clone()))
        .collect();

    // No date dimension → no per-measure date role to resolve. Fall back to the
    // existing blanket cross-fact compatibility check (legacy `bind_with_compat`
    // behaviour) so non-date incompatibilities are still caught.
    if date_dims.is_empty() {
        let reports = check_cross_fact_paths(&bound, enriched);
        return if reports.is_empty() {
            BindResult::Bound(bound)
        } else {
            BindResult::Incompatible(reports)
        };
    }

    // All date hierarchies known in the catalog, with their fact groups — used to
    // report the *valid* date roles for a measure when we reject.
    let catalog_date_hiers = collect_catalog_date_hierarchies(snapshot, enriched);

    let mut rejections: Vec<DateRoleRejection> = Vec::new();

    for measure in &mut bound.measures {
        let m_groups = enriched.groups_for(&measure.unique_name).clone();
        // Conformed measure (no fact binding) → never rejected, no role tag.
        if EnrichedColumnGroups::is_conformed(&m_groups) {
            continue;
        }

        // Find the requested date dimension whose fact intersects this measure.
        let compatible = date_dims.iter().find(|(_, d_groups)| {
            EnrichedColumnGroups::is_conformed(d_groups)
                || m_groups.iter().any(|g| d_groups.contains(g))
        });

        if let Some((dim, _)) = compatible {
            measure.date_role_hierarchy = Some(dim.hierarchy.clone());
            continue;
        }

        // The measure's fact intersects NONE of the requested date roles.
        // Pick a deterministic offending date level to name in the report.
        let Some(offending) = date_dims
            .iter()
            .map(|(d, _)| d)
            .min_by(|a, b| a.unique_name.cmp(&b.unique_name))
        else {
            continue;
        };

        let mut valid: Vec<String> = catalog_date_hiers
            .iter()
            .filter(|(_, groups)| {
                EnrichedColumnGroups::is_conformed(groups)
                    || m_groups.iter().any(|g| groups.contains(g))
            })
            .map(|(hier, _)| hier.clone())
            .collect();
        valid.sort();
        valid.dedup();

        rejections.push(DateRoleRejection {
            code: "cross_fact_date_incompatible".to_string(),
            detail: format!(
                "measure `{}` (fact groups: {}) cannot be grouped on date level `{}:{}` — \
                 that date role serves a different fact; valid date roles for this measure: {}",
                measure.unique_name,
                m_groups.iter().cloned().collect::<Vec<_>>().join(", "),
                offending.hierarchy,
                offending.unique_name,
                if valid.is_empty() {
                    "(none in catalog)".to_string()
                } else {
                    valid.join(", ")
                },
            ),
            measure: measure.unique_name.clone(),
            requested_level: offending.unique_name.clone(),
            valid_hierarchies: valid,
        });
    }

    if !rejections.is_empty() {
        // Deterministic order by measure name.
        rejections.sort_by(|a, b| a.measure.cmp(&b.measure));
        return BindResult::DateRoleIncompatible(rejections);
    }

    // No date-role incompatibility. Defer to the existing cross-fact compat check
    // for any NON-date measure×dimension incompatibilities (reuses the matrix).
    //
    // Date dimensions are intentionally excluded here: in a valid multi-role query
    // each measure is grouped on its *own* date role, so an inventory measure is
    // legitimately disjoint from the `sold_date_*` dimension (and vice-versa). The
    // per-measure pass above already vetted date roles; re-checking them with the
    // blanket pairwise rule would be a false rejection (FR-4).
    let non_date = BoundMqoOutput {
        mqo: bound.mqo.clone(),
        measures: bound.measures.clone(),
        dimensions: bound
            .dimensions
            .iter()
            .filter(|d| !is_date_hierarchy(&d.hierarchy))
            .cloned()
            .collect(),
        calc_group_members: bound.calc_group_members.clone(),
    };
    let reports = check_cross_fact_paths(&non_date, enriched);
    if reports.is_empty() {
        BindResult::Bound(bound)
    } else {
        BindResult::Incompatible(reports)
    }
}

/// Collect every date hierarchy present in the catalog, paired with the union of
/// its levels' fact column-groups. Used to report a measure's *valid* date roles.
fn collect_catalog_date_hierarchies(
    snapshot: &CatalogSnapshot,
    enriched: &EnrichedColumnGroups,
) -> Vec<(String, std::collections::BTreeSet<String>)> {
    use std::collections::BTreeMap;
    let mut by_hier: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    for c in &snapshot.columns {
        if c.kind != "level" {
            continue;
        }
        let Some(hier) = c.hierarchy.as_deref() else {
            continue;
        };
        if !is_date_hierarchy(hier) {
            continue;
        }
        let entry = by_hier.entry(hier.to_string()).or_default();
        for g in enriched.groups_for(&c.unique_name) {
            entry.insert(g.clone());
        }
    }
    by_hier.into_iter().collect()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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

    // ── Per-measure date-role binding + cross-fact date rejection (FR-1/2/3) ──────

    use crate::compat::EnrichedColumnGroups;

    /// Build an `EnrichedColumnGroups` from `(unique_name, &[group])` pairs.
    fn enriched(entries: &[(&str, &[&str])]) -> EnrichedColumnGroups {
        use std::io::Write as _;
        let columns: Vec<Value> = entries
            .iter()
            .map(|(name, groups)| serde_json::json!({ "unique_name": name, "column_group": groups }))
            .collect();
        let catalog = serde_json::json!({ "schema": "enriched-catalog.v1", "columns": columns });
        let mut f = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
        f.write_all(catalog.to_string().as_bytes()).unwrap();
        EnrichedColumnGroups::from_path(f.path()).unwrap()
    }

    /// A snapshot with one inventory measure, one sales measure, and the two
    /// TPC-DS date hierarchies (inventory + sold) each at Month level.
    fn tpcds_like_snapshot() -> CatalogSnapshot {
        CatalogSnapshot {
            columns: vec![
                make_measure("tpcds.inventory_quantity_on_hand", "Inventory Quantity On Hand"),
                make_measure("tpcds.total_store_sales", "Total Store Sales"),
                make_level(
                    "inventory_date_dimensions.[Inventory Calendar Month]",
                    "Inventory Calendar Month",
                    "inventory_date_dimensions",
                    "Inventory Calendar Month",
                ),
                make_level(
                    "sold_date_dimensions.[Sold Calendar Month]",
                    "Sold Calendar Month",
                    "sold_date_dimensions",
                    "Sold Calendar Month",
                ),
            ],
            ..CatalogSnapshot::default()
        }
    }

    fn tpcds_enriched() -> EnrichedColumnGroups {
        enriched(&[
            ("tpcds.inventory_quantity_on_hand", &["inventory"]),
            ("tpcds.total_store_sales", &["store_sales"]),
            ("inventory_date_dimensions.[Inventory Calendar Month]", &["inventory"]),
            ("sold_date_dimensions.[Sold Calendar Month]", &["store_sales", "catalog_sales", "web_sales"]),
        ])
    }

    fn dr_mqo(measures: &[&str], dims: &[(&str, &str)]) -> Mqo {
        Mqo {
            model: "tpcds".to_string(),
            measures: measures
                .iter()
                .map(|m| MeasureRef { unique_name: (*m).to_string() })
                .collect(),
            dimensions: dims
                .iter()
                .map(|(h, l)| LevelSelection { hierarchy: (*h).to_string(), level: (*l).to_string() })
                .collect(),
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
        }
    }

    /// AC-1 unit: inventory + sales measure under `Sold Calendar Month` ONLY →
    /// the inventory measure is flagged `cross_fact_date_incompatible`.
    #[test]
    fn date_role_inventory_under_sold_month_is_rejected() {
        let snapshot = tpcds_like_snapshot();
        let e = tpcds_enriched();
        let mqo = dr_mqo(
            &["tpcds.inventory_quantity_on_hand", "tpcds.total_store_sales"],
            &[("sold_date_dimensions", "Sold Calendar Month")],
        );
        match bind_with_date_roles(&mqo, &snapshot, &e) {
            BindResult::DateRoleIncompatible(rs) => {
                assert_eq!(rs.len(), 1, "only the inventory measure should be flagged");
                assert_eq!(rs[0].code, "cross_fact_date_incompatible");
                assert_eq!(rs[0].measure, "tpcds.inventory_quantity_on_hand");
                assert_eq!(rs[0].requested_level, "sold_date_dimensions.[Sold Calendar Month]");
                assert!(
                    rs[0].valid_hierarchies.contains(&"inventory_date_dimensions".to_string()),
                    "valid roles must name the inventory date hierarchy: {:?}",
                    rs[0].valid_hierarchies
                );
            }
            other => panic!("expected DateRoleIncompatible, got {other:?}"),
        }
    }

    /// AC-1 unit (per-measure binding): inventory + sales each under their OWN
    /// date role → Bound, each measure tagged with its date_role_hierarchy.
    #[test]
    fn date_role_per_measure_binding_both_roles_present() {
        let snapshot = tpcds_like_snapshot();
        let e = tpcds_enriched();
        let mqo = dr_mqo(
            &["tpcds.inventory_quantity_on_hand", "tpcds.total_store_sales"],
            &[
                ("inventory_date_dimensions", "Inventory Calendar Month"),
                ("sold_date_dimensions", "Sold Calendar Month"),
            ],
        );
        match bind_with_date_roles(&mqo, &snapshot, &e) {
            BindResult::Bound(b) => {
                let inv = b.measures.iter().find(|m| m.unique_name.contains("inventory")).unwrap();
                let sales = b.measures.iter().find(|m| m.unique_name.contains("store_sales")).unwrap();
                assert_eq!(inv.date_role_hierarchy.as_deref(), Some("inventory_date_dimensions"));
                assert_eq!(sales.date_role_hierarchy.as_deref(), Some("sold_date_dimensions"));
            }
            other => panic!("expected Bound with per-measure date roles, got {other:?}"),
        }
    }

    /// AC-2 unit: ONLY sales measures under `Sold Calendar Month` → binds normally.
    #[test]
    fn date_role_sales_only_under_sold_month_binds() {
        let snapshot = tpcds_like_snapshot();
        let e = tpcds_enriched();
        let mqo = dr_mqo(
            &["tpcds.total_store_sales"],
            &[("sold_date_dimensions", "Sold Calendar Month")],
        );
        match bind_with_date_roles(&mqo, &snapshot, &e) {
            BindResult::Bound(b) => {
                assert_eq!(
                    b.measures[0].date_role_hierarchy.as_deref(),
                    Some("sold_date_dimensions")
                );
            }
            other => panic!("expected Bound (sales-only), got {other:?}"),
        }
    }

    /// AC-3 unit: ONLY inventory measures under `Inventory Calendar Month` → binds normally.
    #[test]
    fn date_role_inventory_only_under_inventory_month_binds() {
        let snapshot = tpcds_like_snapshot();
        let e = tpcds_enriched();
        let mqo = dr_mqo(
            &["tpcds.inventory_quantity_on_hand"],
            &[("inventory_date_dimensions", "Inventory Calendar Month")],
        );
        match bind_with_date_roles(&mqo, &snapshot, &e) {
            BindResult::Bound(b) => {
                assert_eq!(
                    b.measures[0].date_role_hierarchy.as_deref(),
                    Some("inventory_date_dimensions")
                );
            }
            other => panic!("expected Bound (inventory-only), got {other:?}"),
        }
    }

    /// FR-4 false-positive guard: no date dimension at all → never rejected,
    /// no date role tagged.
    #[test]
    fn date_role_no_date_dimension_binds_unchanged() {
        let snapshot = tpcds_like_snapshot();
        let e = tpcds_enriched();
        let mqo = dr_mqo(&["tpcds.total_store_sales"], &[]);
        match bind_with_date_roles(&mqo, &snapshot, &e) {
            BindResult::Bound(b) => {
                assert!(b.measures[0].date_role_hierarchy.is_none());
            }
            other => panic!("expected Bound (no date dim), got {other:?}"),
        }
    }

    /// FR-4 guard: a conformed measure (no fact binding) is never date-rejected.
    #[test]
    fn date_role_conformed_measure_not_rejected() {
        let mut snapshot = tpcds_like_snapshot();
        snapshot.columns.push(make_measure("tpcds.conformed_count", "Conformed Count"));
        // conformed_count has NO enriched entry → treated as conformed (fail-open).
        let e = tpcds_enriched();
        let mqo = dr_mqo(
            &["tpcds.conformed_count"],
            &[("sold_date_dimensions", "Sold Calendar Month")],
        );
        match bind_with_date_roles(&mqo, &snapshot, &e) {
            BindResult::Bound(b) => {
                assert!(b.measures[0].date_role_hierarchy.is_none());
            }
            other => panic!("expected Bound (conformed measure), got {other:?}"),
        }
    }

    /// is_date_hierarchy must recognise the TPC-DS date roles and reject non-date dims.
    #[test]
    fn is_date_hierarchy_recognises_date_roles() {
        assert!(is_date_hierarchy("sold_date_dimensions"));
        assert!(is_date_hierarchy("inventory_date_dimensions"));
        assert!(is_date_hierarchy("sold_date_week_hierarchy"));
        assert!(!is_date_hierarchy("store_dimension"));
        assert!(!is_date_hierarchy("customer_dimension"));
    }
}
