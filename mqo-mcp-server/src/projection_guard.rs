//! Projection cardinality guard — pre-execution estimate for measureless
//! (projection) MQOs.
//!
//! # Why this exists
//!
//! A projection MQO (`SELECT DISTINCT`-style, no measures) over a high-cardinality
//! level (e.g. `Customer Id`) would trigger the engine's row cap, spending credits
//! and returning 0 rows (`rowLimitAdvisory`). This module estimates the distinct
//! cardinality *before* execution and either permits it (under cap) or declines
//! with a typed `projection_too_large` error.
//!
//! # Estimation strategy
//!
//! - For each dimension level in the projection, look up its member count from
//!   the catalog column's `domain` field (length of the enumerated domain when
//!   present).
//! - If a `Filter::Member` or `Filter::MemberLevel` targets that level's
//!   hierarchy, use the listed member count as the selectivity estimate.
//! - If a `Filter::Range` targets that level, estimate a fractional selectivity.
//! - The product of per-level estimates is the total distinct-row estimate.
//! - A level with no known member count is treated as unknown → decline
//!   (conservative fail-safe).
//!
//! # Cross-hierarchy product (FR-4)
//!
//! The product of independent-hierarchy estimates is a loose upper bound: the
//! true distinct (First Name, Gender) count is at most max(First Name, Gender),
//! not 5,126 × 2 = 10,488.  When the product exceeds the cap but all individual
//! per-hierarchy estimates are ≤ cap, the product is **advisory**: we cap the
//! running product at the budget and proceed, letting the runtime `row_cap_tripped`
//! be the hard floor (PRD-mqo-projection-handle-over-cap, FR-4/OQ-1).
//!
//! # Cross-hierarchy filter selectivity (FR-5)
//!
//! A filter targeting a hierarchy not represented in the projection still narrows
//! the result via auto-exist / semijoin semantics.  When such a filter exists and
//! the current total product is unconstrained, a conservative 1/10 selectivity
//! reduction is applied.  When the post-filter count cannot be estimated (e.g. the
//! hierarchy is entirely unknown) the estimate is demoted to advisory (capped at
//! budget, proceeds).
//!
//! # Integration note
//!
//! `mqo-attribute-projection` defines a stub of this same function that always
//! returns `Ok(())`. When the branches integrate, this real implementation
//! replaces that stub. The signature MUST match exactly.

#![forbid(unsafe_code)]

use mqo_catalog_binder::catalog::CatalogSnapshot;
use mqo_spec::{Filter, FilterGroupOp, Mqo, RangeBound};

/// A projected level resolved against the catalog: its hierarchy, canonical
/// `unique_name`, and known total member count (`None` when the catalog has no
/// cardinality/domain for it).
struct ResolvedLevel {
    hierarchy: String,
    level_un: String,
    total_members: Option<u64>,
}

// ── Public types ──────────────────────────────────────────────────────────────

/// A projection whose estimated distinct cardinality exceeds the configured cap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionTooLarge {
    /// The first level whose contribution pushed the estimate over the cap.
    pub level: String,
    /// The total estimated distinct-row count for the whole projection.
    pub estimate: u64,
    /// The configured cap that was exceeded.
    pub cap: usize,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Pre-execution cardinality check for projection MQOs.
///
/// Returns `Ok(())` when the estimated distinct cardinality is within `cap`,
/// meaning the projection may proceed (the caller handles pagination).
///
/// Returns `Err(ProjectionTooLarge)` when the estimate exceeds `cap` or when
/// the cardinality of any dimension level is unknown (fail-safe, FR-5).
///
/// # Arguments
///
/// * `mqo`     — the MQO to check (use its `dimensions` and `filters`).
/// * `catalog` — the catalog snapshot that carries per-level `domain` counts.
/// * `cap`     — maximum allowed distinct-row estimate.  `0` means "always
///   decline" (operator's explicit choice).
///
/// # Edge cases
///
/// * `cap == 0` → every projection declines regardless of size.
/// * No dimensions → trivially `Ok(())` (nothing to scan; the empty set has 0 rows).
pub fn check_projection_cardinality(
    mqo: &Mqo,
    catalog: &CatalogSnapshot,
    cap: usize,
) -> Result<(), ProjectionTooLarge> {
    // A zero-cap always declines.  An empty projection is trivially safe.
    if mqo.dimensions.is_empty() {
        return Ok(());
    }

    // Build an estimate for each dimension level.
    // Resolve each projected level to (hierarchy, canonical unique_name,
    // total member count). The member count prefers the persisted true
    // `cardinality` (LEVEL_CARDINALITY from MDSCHEMA_LEVELS, uncapped) and falls
    // back to `domain.len()` (back-compat with old snapshots / levels lacking
    // cluster metadata).
    let resolved: Vec<ResolvedLevel> = mqo
        .dimensions
        .iter()
        .map(|dim| {
            let maybe_entry = catalog
                .columns
                .iter()
                .find(|col| col.kind == "level" && level_matches(col, &dim.hierarchy, &dim.level));
            let level_un = maybe_entry
                .map(|e| e.unique_name.clone())
                .unwrap_or_else(|| format!("{}.{}", dim.hierarchy, dim.level));
            let total_members: Option<u64> = maybe_entry.and_then(|e| {
                if let Some(card) = e.cardinality {
                    if card > 0 {
                        return Some(card);
                    }
                }
                e.domain.as_ref().map(|d| d.len() as u64).filter(|&n| n > 0)
            });
            ResolvedLevel {
                hierarchy: dim.hierarchy.clone(),
                level_un,
                total_members,
            }
        })
        .collect();

    // Group projected levels by hierarchy, preserving first-seen order.
    //
    // KEY CORRECTNESS POINT (cardinality-estimate-fix): attributes within ONE
    // hierarchy are functionally dependent — each member of the finest level
    // determines the coarser levels' values (a store has exactly one manager,
    // one floor space). Projecting k levels of the SAME hierarchy therefore
    // yields at most (finest-level cardinality) distinct rows, NOT the product
    // of the per-level cardinalities. The prior implementation multiplied every
    // level unconditionally, over-estimating same-hierarchy attribute
    // projections by orders of magnitude (e.g. Store Name × Store Manager ×
    // Store Floor Space ≈ 9e5 for ~1e3 stores). Only INDEPENDENT hierarchies
    // legitimately cross-multiply. A filter on ANY level of a hierarchy
    // constrains the WHOLE group (the levels co-vary).
    let mut order: Vec<&str> = Vec::new();
    let mut groups: std::collections::HashMap<&str, Vec<&ResolvedLevel>> =
        std::collections::HashMap::new();
    for rl in &resolved {
        groups
            .entry(rl.hierarchy.as_str())
            .or_insert_with(|| {
                order.push(rl.hierarchy.as_str());
                Vec::new()
            })
            .push(rl);
    }

    // Pre-compute the set of projected hierarchies for cross-hierarchy filter
    // selectivity (FR-5). Used both inside the loop (per-group check) and after.
    let projected_hiers: std::collections::HashSet<&str> =
        order.iter().map(|h| *h).collect();

    let mut total_estimate: u64 = 1;
    // Track whether the running product was ever capped advisory (FR-4):
    // when each individual hierarchy's estimate is ≤ cap but their product
    // exceeds cap, we demote to advisory and proceed.
    let mut product_was_capped_advisory = false;
    for hier in &order {
        let levels = &groups[hier];

        // Base: the finest projected level bounds the row count → MAX of the
        // known member counts across this hierarchy's projected levels.
        let base: Option<u64> = levels.iter().filter_map(|l| l.total_members).max();

        // A filter on any level of this hierarchy constrains the whole group.
        let filter_est = hierarchy_filter_estimate(hier, levels, &mqo.filters, cap);

        // A filter can only reduce the base. Combine accordingly.
        let group_estimate = match (base, filter_est) {
            (Some(b), Some(f)) => LevelEstimate::Known(b.min(f)),
            (Some(b), None) => LevelEstimate::Known(b),
            (None, Some(f)) => LevelEstimate::Known(f),
            (None, None) => LevelEstimate::Unknown,
        };

        match group_estimate {
            LevelEstimate::Known(n) => {
                let per_group = n.max(1);
                // FR-5 early application: if this projected hierarchy's base
                // estimate exceeds the cap but there are cross-hierarchy filters
                // that could narrow it, apply them NOW before the per-group cap
                // check, so a `Product Current Price > 70` filter on a non-projected
                // hierarchy can reduce an over-cap `Item Product Name` estimate.
                let per_group = if cap > 0 && per_group > cap as u64 {
                    let tentative_total = total_estimate.saturating_mul(per_group);
                    if let Some(reduced) = cross_hierarchy_filter_selectivity(
                        &projected_hiers,
                        &mqo.filters,
                        tentative_total,
                        cap,
                    ) {
                        // Reduced estimate fits: proceed with it.
                        if reduced <= cap as u64 {
                            total_estimate = reduced;
                            product_was_capped_advisory = true;
                            // Skip the normal per_group accumulation below.
                            continue;
                        }
                    }
                    // Cross-hierarchy filters could not save it — hard reject.
                    return Err(ProjectionTooLarge {
                        level: levels
                            .last()
                            .map(|l| l.level_un.clone())
                            .unwrap_or_else(|| (*hier).to_string()),
                        estimate: per_group,
                        cap,
                    });
                } else {
                    per_group
                };
                // FR-4: cross-hierarchy product advisory cap.
                // If this hierarchy's own estimate is within budget, but
                // multiplying it pushes the running product over the budget,
                // cap the product at the budget and treat the estimate as
                // advisory — the runtime row_cap_tripped is the hard bound.
                let new_product = total_estimate.saturating_mul(per_group);
                if cap > 0 && new_product > cap as u64 {
                    // Each factor is within cap but the product is not →
                    // advisory: cap at budget and proceed.
                    total_estimate = cap as u64;
                    product_was_capped_advisory = true;
                } else {
                    total_estimate = new_product;
                }
            }
            LevelEstimate::Unknown => {
                // Fail safe: unknown cardinality → decline.
                return Err(ProjectionTooLarge {
                    level: "cardinality_unknown".to_string(),
                    estimate: cap.saturating_add(1) as u64,
                    cap,
                });
            }
        }
    }

    // cap == 0 → always decline (operator's explicit "never execute" setting).
    if cap == 0 {
        return Err(ProjectionTooLarge {
            level: mqo
                .dimensions
                .first()
                .map(|d| format!("{}.{}", d.hierarchy, d.level))
                .unwrap_or_default(),
            estimate: total_estimate,
            cap,
        });
    }

    // FR-5 post-loop: Apply selectivity from cross-hierarchy filters if the
    // product was not already advisory.  Handles the normal case where the
    // per-group estimates were within cap and no early FR-5 path was taken.
    if !product_was_capped_advisory {
        let cross_filter_reduction = cross_hierarchy_filter_selectivity(
            &projected_hiers,
            &mqo.filters,
            total_estimate,
            cap,
        );
        if let Some(reduced) = cross_filter_reduction {
            total_estimate = reduced;
        }
    }

    // Final check: if after FR-4 advisory cap and FR-5 reduction the estimate
    // is still > cap, hard reject.
    if cap > 0 && total_estimate > cap as u64 {
        return Err(ProjectionTooLarge {
            level: mqo
                .dimensions
                .first()
                .map(|d| format!("{}.{}", d.hierarchy, d.level))
                .unwrap_or_default(),
            estimate: total_estimate,
            cap,
        });
    }

    // Estimate ≤ cap — projection is within budget.
    Ok(())
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Does catalog column `col` correspond to the MQO dimension
/// (`dim_hierarchy`, `dim_level`)?
///
/// `describe_model` keys a level by its `unique_name` (e.g.
/// `"ship_mode.[Carrier]"`).  A client therefore commonly passes the dimension
/// `level` in one of several forms, and the guard MUST resolve all of them to
/// the same catalog column — otherwise a domain-bearing level looks
/// "cardinality unknown" and the projection is wrongly declined.  Accepted
/// forms for `dim_level`:
///
/// * the bare level name — `"Carrier"`            (matches `col.level`)
/// * the bracketed level name — `"[Carrier]"`     (matches `col.level` w/ brackets)
/// * the full unique_name — `"ship_mode.[Carrier]"` (matches `col.unique_name`)
///
/// plus the reconstructed `"{dim_hierarchy}.{dim_level}"` against
/// `col.unique_name` (back-compat with the bracketed `unique_name` form).
fn level_matches(col: &mqo_catalog_binder::catalog::ColumnEntry, dim_hierarchy: &str, dim_level: &str) -> bool {
    // 1. Client passed the full unique_name as `level`.
    if col.unique_name == dim_level {
        return true;
    }
    // 2. Reconstructed hierarchy.level matches the unique_name
    //    (handles bracketed level e.g. `level = "[Carrier]"`).
    if col.unique_name == format!("{dim_hierarchy}.{dim_level}") {
        return true;
    }
    // 3. Hierarchy + level fields match (bare or bracketed level name).
    let hier_ok = col.hierarchy.as_deref() == Some(dim_hierarchy);
    if hier_ok {
        if let Some(catalog_level) = col.level.as_deref() {
            // Strip surrounding brackets from the supplied level so `[Carrier]`
            // and `Carrier` both match a catalog `level` of `Carrier`.
            let bare = dim_level.strip_prefix('[').and_then(|s| s.strip_suffix(']')).unwrap_or(dim_level);
            if catalog_level == dim_level || catalog_level == bare {
                return true;
            }
        }
    }
    false
}

/// The result of estimating cardinality for a hierarchy group.
enum LevelEstimate {
    /// A concrete (possibly filter-adjusted) member count.
    Known(u64),
    /// No member count is available in the catalog.
    Unknown,
}

/// Estimate the row count contributed by the filters that target one
/// hierarchy group, or `None` when no filter constrains it (caller falls back
/// to the group's base cardinality).
///
/// A filter on ANY level of the hierarchy constrains the whole group, because
/// the projected levels co-vary. When several filters match, take the most
/// selective (intersection ≤ the smallest). `Range` filters are matched against
/// the specific projected level's `unique_name` and total member count.
fn hierarchy_filter_estimate(
    hierarchy: &str,
    levels: &[&ResolvedLevel],
    filters: &[Filter],
    cap: usize,
) -> Option<u64> {
    let mut best: Option<u64> = None;
    for f in filters {
        let est = match f {
            Filter::Group { op, filters: inner } => {
                // One level of nesting. Collect inner leaf estimates targeting
                // this hierarchy and combine: AND ⇒ intersection (≤ min),
                // OR ⇒ union (≤ sum).
                let inner_ests: Vec<u64> = inner
                    .iter()
                    .filter_map(|inf| single_filter_estimate(hierarchy, levels, inf, cap))
                    .collect();
                if inner_ests.is_empty() {
                    None
                } else {
                    Some(match op {
                        FilterGroupOp::And => *inner_ests.iter().min().unwrap_or(&0),
                        FilterGroupOp::Or => inner_ests.iter().copied().sum(),
                    })
                }
            }
            leaf => single_filter_estimate(hierarchy, levels, leaf, cap),
        };
        if let Some(e) = est {
            best = Some(best.map_or(e, |b| b.min(e)));
        }
    }
    best
}

/// Estimate the row count from a single leaf filter, if it targets this
/// hierarchy (or one of its projected levels). `None` when the filter does not
/// apply here.
fn single_filter_estimate(
    hierarchy: &str,
    levels: &[&ResolvedLevel],
    filter: &Filter,
    cap: usize,
) -> Option<u64> {
    match filter {
        // An IN-list on the hierarchy: the listed members are the result set.
        Filter::Member { hierarchy: fh, members } if fh == hierarchy => Some(members.len() as u64),
        // An explicit-level IN-list on this hierarchy. `exclude` (NOT-IN) does
        // not reduce to a small set, so leave the group bounded by its base.
        Filter::MemberLevel {
            hierarchy: fh,
            members,
            exclude,
            ..
        } if fh == hierarchy => {
            if *exclude {
                None
            } else {
                Some(members.len() as u64)
            }
        }
        // A range on a level of this hierarchy.
        Filter::Range { level: fl, lo, hi } => {
            if let Some(target) = levels.iter().find(|l| l.level_un == *fl) {
                // Range on a PROJECTED level: use its fractional selectivity.
                match estimate_range_selectivity(lo, hi, target.total_members) {
                    Some(sel) => target.total_members.map(|t| {
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let est = ((t as f64) * sel).ceil() as u64;
                        est.max(1)
                    }),
                    // Range with unknown total → bounded intent; cap is the
                    // conservative upper bound so it may pass.
                    None if target.total_members.is_none() => Some(cap as u64),
                    None => None,
                }
            } else {
                // Range on a NON-projected level of THIS hierarchy (e.g.
                // `Product Current Price > 70` filtering an `Item Product Name`
                // projection — both in product_dimension). A filter on ANY level of
                // a hierarchy constrains the whole group (the levels co-vary), so the
                // earlier "projected level only" guard under-applied it and left the
                // estimate at the full unfiltered domain (the price-above-70 gap).
                // We don't know the filtered level's cardinality, so apply the same
                // conservative 1/10 selectivity used for cross-hierarchy Range filters
                // (FR-5) to the group's base; the runtime materialization budget
                // (row_cap_tripped) remains the hard bound if the estimate is optimistic.
                let filter_hier = fl.split_once(".[").map(|(h, _)| h).unwrap_or(fl.as_str());
                if filter_hier == hierarchy {
                    let base = levels.iter().filter_map(|l| l.total_members).max();
                    base.map(|b| (b / 10).max(1))
                } else {
                    None
                }
            }
        }
        _ => None,
    }
}

/// Apply selectivity from filters that target hierarchies *not* represented in
/// the projection (FR-5: cross-hierarchy filter selectivity).
///
/// Auto-exist / semijoin semantics mean a `Product Current Price > 70` filter
/// constrains the projected `Item Product Name` set even though the two live on
/// different hierarchies.  The current guard ignores these filters, over-counting
/// by the full unfiltered domain.
///
/// Conservative rules (chosen to be cheap and never false-positive):
/// - `Range` filter on a non-projected hierarchy → 1/10 selectivity applied to
///   the current `total_estimate`.
/// - `Member` or `MemberLevel` (IN-list, non-exclude) → the member count is an
///   upper bound; use `min(total_estimate, member_count)`.
/// - If no cross-hierarchy filter is present, returns `None` (no change).
/// - If the resulting estimate is already ≤ cap, returns `Some(estimate)` to
///   allow the projection to proceed.
fn cross_hierarchy_filter_selectivity(
    projected_hiers: &std::collections::HashSet<&str>,
    filters: &[Filter],
    current_estimate: u64,
    _cap: usize,
) -> Option<u64> {
    let mut reduced = current_estimate;
    let mut any_cross = false;

    for f in filters {
        match f {
            Filter::Range { level: fl, lo: _, hi: _ } => {
                // Derive the hierarchy from the level unique_name ("hier.[Level]").
                let filter_hier = fl.split_once(".[")
                    .map(|(h, _)| h)
                    .unwrap_or(fl.as_str());
                if !projected_hiers.contains(filter_hier) {
                    // Cross-hierarchy Range: apply 1/10 selectivity (conservative).
                    reduced = (reduced / 10).max(1);
                    any_cross = true;
                }
            }
            Filter::Member { hierarchy: fh, members } if !projected_hiers.contains(fh.as_str()) => {
                // Cross-hierarchy Member IN-list: result ≤ member count.
                let member_bound = members.len() as u64;
                if member_bound < reduced {
                    reduced = member_bound.max(1);
                }
                any_cross = true;
            }
            Filter::MemberLevel {
                hierarchy: fh,
                members,
                exclude,
                ..
            } if !projected_hiers.contains(fh.as_str()) && !exclude => {
                let member_bound = members.len() as u64;
                if member_bound < reduced {
                    reduced = member_bound.max(1);
                }
                any_cross = true;
            }
            Filter::Group { filters: inner, .. } => {
                // Recurse one level.
                if let Some(r) = cross_hierarchy_filter_selectivity(
                    projected_hiers, inner, reduced, _cap,
                ) {
                    if r < reduced {
                        reduced = r;
                        any_cross = true;
                    }
                }
            }
            _ => {}
        }
    }

    if any_cross { Some(reduced) } else { None }
}

/// Estimate range selectivity as a fraction `[0, 1]` when both bounds are
/// numeric and the domain size is known.
///
/// Returns `None` when the computation is not possible (mixed types, etc.).
fn estimate_range_selectivity(
    lo: &RangeBound,
    hi: &RangeBound,
    total_members: Option<u64>,
) -> Option<f64> {
    let (lo_n, hi_n) = (lo.as_f64()?, hi.as_f64()?);
    let total = total_members? as f64;
    if total <= 0.0 {
        return None;
    }
    // Use range width / total as a fraction; clamp to [ε, 1].
    let range_width = (hi_n - lo_n).abs();
    // Interpret range_width as "number of distinct integer values covered"
    // relative to the domain size.  Clamp to avoid divide-by-zero.
    let sel = (range_width + 1.0) / total;
    Some(sel.clamp(1.0 / total, 1.0))
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mqo_catalog_binder::catalog::{CatalogSnapshot, ColumnEntry};
    use mqo_spec::{LevelSelection, MeasureRef, Mqo};

    fn make_catalog_with_level(hierarchy: &str, level: &str, domain_size: usize) -> CatalogSnapshot {
        let level_name = format!("[{level}]");
        let unique_name = format!("{hierarchy}.{level_name}");
        let mut col = ColumnEntry {
            unique_name: unique_name.clone(),
            label: level.to_string(),
            kind: "level".to_string(),
            hierarchy: Some(hierarchy.to_string()),
            level: Some(level.to_string()),
            ..Default::default()
        };
        if domain_size > 0 {
            col.domain = Some((0..domain_size).map(|i| i.to_string()).collect());
        }
        CatalogSnapshot {
            columns: vec![col],
            ..Default::default()
        }
    }

    fn make_projection_mqo(hierarchy: &str, level: &str) -> Mqo {
        Mqo {
            model: "sales".to_string(),
            measures: vec![MeasureRef { unique_name: "placeholder".to_string() }],
            dimensions: vec![LevelSelection {
                hierarchy: hierarchy.to_string(),
                level: level.to_string(),
            }],
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
            projection: false,
        }
    }

    // AC-1: high-cardinality level → ProjectionTooLarge returned, no execution
    #[test]
    fn ac1_high_cardinality_declines() {
        let catalog = make_catalog_with_level("customer_dimension", "Customer Id", 50_000);
        let mqo = make_projection_mqo("customer_dimension", "Customer Id");
        let result = check_projection_cardinality(&mqo, &catalog, 10_000);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.cap, 10_000);
        assert!(err.estimate > 10_000);
    }

    // AC-2: low-cardinality level → Ok(())
    #[test]
    fn ac2_low_cardinality_passes() {
        let catalog = make_catalog_with_level("store_dimension", "State", 50);
        let mqo = make_projection_mqo("store_dimension", "State");
        let result = check_projection_cardinality(&mqo, &catalog, 10_000);
        assert!(result.is_ok());
    }

    // AC-3: selective IN-list filter reduces estimate below cap → Ok(())
    #[test]
    fn ac3_selective_filter_passes() {
        // 50,000 customers but only 5 listed in the filter.
        let catalog = make_catalog_with_level("customer_dimension", "Customer Id", 50_000);
        let mut mqo = make_projection_mqo("customer_dimension", "Customer Id");
        mqo.filters.push(Filter::Member {
            hierarchy: "customer_dimension".to_string(),
            members: vec![
                "C1".to_string(),
                "C2".to_string(),
                "C3".to_string(),
                "C4".to_string(),
                "C5".to_string(),
            ],
        });
        let result = check_projection_cardinality(&mqo, &catalog, 10_000);
        assert!(result.is_ok(), "5-member filter should pass; got: {result:?}");
    }

    // AC-4: operator-set cap respected
    #[test]
    fn ac4_operator_cap_respected() {
        // 100 members, but the operator set a very low cap of 50.
        let catalog = make_catalog_with_level("product_dimension", "Brand", 100);
        let mqo = make_projection_mqo("product_dimension", "Brand");
        let result = check_projection_cardinality(&mqo, &catalog, 50);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.cap, 50);

        // Same with a cap of 200 — should pass.
        let result2 = check_projection_cardinality(&mqo, &catalog, 200);
        assert!(result2.is_ok());
    }

    // AC-5: unknown cardinality → fails safe (Err)
    #[test]
    fn ac5_unknown_cardinality_declines() {
        // Level with no domain (unknown cardinality).
        let catalog = make_catalog_with_level("store_dimension", "Store Id", 0);
        let mqo = make_projection_mqo("store_dimension", "Store Id");
        let result = check_projection_cardinality(&mqo, &catalog, 10_000);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.level, "cardinality_unknown");
    }

    // Extra: empty dimensions → always Ok
    #[test]
    fn empty_dimensions_always_ok() {
        let catalog = CatalogSnapshot::default();
        let mqo = Mqo {
            model: "sales".to_string(),
            measures: vec![MeasureRef { unique_name: "placeholder".to_string() }],
            dimensions: vec![],
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
            projection: false,
        };
        assert!(check_projection_cardinality(&mqo, &catalog, 10_000).is_ok());
    }

    // Extra: cap == 0 → always decline
    #[test]
    fn cap_zero_always_declines() {
        let catalog = make_catalog_with_level("store_dimension", "State", 5);
        let mqo = make_projection_mqo("store_dimension", "State");
        let result = check_projection_cardinality(&mqo, &catalog, 0);
        assert!(result.is_err());
    }

    /// Build a catalog column shaped like the real `tpcds_catalog.json` /
    /// live-ingested snapshot: `unique_name` is `"{hierarchy}.[{Level}]"`, the
    /// `level` field is the BARE name (no brackets), and the member set lives in
    /// the `domain` array (there is no separate count field).
    fn make_realistic_catalog(hierarchy: &str, level: &str, domain_size: usize) -> CatalogSnapshot {
        let unique_name = format!("{hierarchy}.[{level}]");
        let mut col = ColumnEntry {
            unique_name,
            label: level.to_string(),
            kind: "level".to_string(),
            hierarchy: Some(hierarchy.to_string()),
            level: Some(level.to_string()), // BARE — mirrors the real fixture
            ..Default::default()
        };
        if domain_size > 0 {
            col.domain = Some((0..domain_size).map(|i| format!("m{i}")).collect());
        }
        CatalogSnapshot { columns: vec![col], ..Default::default() }
    }

    // Regression (cardinality-guard-fix): a level the catalog has a `domain` for
    // must be resolved — and its count read from `domain.len()` — REGARDLESS of
    // which level-name form the client supplies.  Previously, passing the full
    // unique_name (what describe_model advertises) made the guard miss the
    // domain-bearing column and wrongly decline with `cardinality_unknown`.
    #[test]
    fn domain_resolves_for_all_level_name_forms() {
        // 20-member domain (like ship_mode.[Carrier]); cap 10_000 → must pass.
        let catalog = make_realistic_catalog("ship_mode", "Carrier", 20);
        for level_form in ["Carrier", "[Carrier]", "ship_mode.[Carrier]"] {
            let mqo = make_projection_mqo("ship_mode", level_form);
            let result = check_projection_cardinality(&mqo, &catalog, 10_000);
            assert!(
                result.is_ok(),
                "level form {level_form:?} should resolve to the 20-member domain and pass, got: {result:?}"
            );
        }
    }

    // Regression: a 2-member domain (like customer_demographics.[Gender]) passes
    // even when the client sends the full unique_name as the level.
    #[test]
    fn small_domain_via_unique_name_passes() {
        let catalog = make_realistic_catalog("customer_demographics", "Gender", 2);
        let mqo = make_projection_mqo("customer_demographics", "customer_demographics.[Gender]");
        assert!(check_projection_cardinality(&mqo, &catalog, 10_000).is_ok());
    }

    // Regression: count comes from `domain.len()`, so a domain larger than the
    // cap still declines (fail-safe preserved) for every level-name form.
    #[test]
    fn large_domain_declines_for_all_level_name_forms() {
        let catalog = make_realistic_catalog("customer_dimension", "Customer Id", 50_000);
        for level_form in ["Customer Id", "[Customer Id]", "customer_dimension.[Customer Id]"] {
            let mqo = make_projection_mqo("customer_dimension", level_form);
            let result = check_projection_cardinality(&mqo, &catalog, 10_000);
            assert!(
                result.is_err(),
                "level form {level_form:?} (50k domain) must decline, got: {result:?}"
            );
            assert!(result.unwrap_err().estimate > 10_000);
        }
    }

    // Regression: a level the catalog has NO domain for is still unknown →
    // fail-safe decline, regardless of level-name form.
    #[test]
    fn no_domain_still_declines() {
        let catalog = make_realistic_catalog("store_dimension", "Store Id", 0);
        let mqo = make_projection_mqo("store_dimension", "store_dimension.[Store Id]");
        let err = check_projection_cardinality(&mqo, &catalog, 10_000).unwrap_err();
        assert_eq!(err.level, "cardinality_unknown");
    }

    // Extra: MemberLevel filter selectivity
    #[test]
    fn member_level_filter_is_selective() {
        let catalog = make_catalog_with_level("customer_dimension", "Gender", 2);
        let mut mqo = make_projection_mqo("customer_dimension", "Gender");
        mqo.filters.push(Filter::MemberLevel {
            hierarchy: "customer_dimension".to_string(),
            level: "customer_dimension.[Gender]".to_string(),
            members: vec!["M".to_string()],
            exclude: false,
        });
        let result = check_projection_cardinality(&mqo, &catalog, 10_000);
        assert!(result.is_ok());
    }

    // ── New tests: cardinality field (PRD-mqo-cardinality-from-level-count) ─────

    /// Build a catalog column that has BOTH a truncated domain AND a known true
    /// cardinality (the cardinality field wins over domain.len()).
    fn make_catalog_with_known_cardinality(
        hierarchy: &str,
        level: &str,
        domain_size: usize,
        cardinality: Option<u64>,
    ) -> CatalogSnapshot {
        let unique_name = format!("{hierarchy}.[{level}]");
        let mut col = ColumnEntry {
            unique_name,
            label: level.to_string(),
            kind: "level".to_string(),
            hierarchy: Some(hierarchy.to_string()),
            level: Some(level.to_string()),
            cardinality,
            ..Default::default()
        };
        if domain_size > 0 {
            col.domain = Some((0..domain_size).map(|i| format!("m{i}")).collect());
        }
        CatalogSnapshot { columns: vec![col], ..Default::default() }
    }

    // PRD AC-1: level with cardinality: Some(10436) and a truncated 50-member
    // domain (like Sold Calendar Week) → projection declines projection_too_large
    // with an estimate ≈ 10,436 (NOT ≈ 50 from the truncated domain).
    #[test]
    fn ac_card1_known_large_cardinality_declines_with_true_estimate() {
        // domain is only 50 (truncated), but true cardinality is 10,436.
        let catalog = make_catalog_with_known_cardinality(
            "sold_date_week_hierarchy",
            "Sold Calendar Week",
            50,
            Some(10_436),
        );
        let mqo = make_projection_mqo("sold_date_week_hierarchy", "Sold Calendar Week");
        let result = check_projection_cardinality(&mqo, &catalog, 1_000);
        assert!(result.is_err(), "known-large level must decline");
        let err = result.unwrap_err();
        // estimate should reflect the true cardinality (~10,436), not the
        // truncated domain (50).
        assert!(
            err.estimate >= 10_436,
            "estimate should be ≥ true cardinality (10,436), got {}",
            err.estimate
        );
        // NOT cardinality_unknown — we have a real count.
        assert_ne!(err.level, "cardinality_unknown");
    }

    // PRD AC-3: level with cardinality: Some(20) (small known count) → admitted.
    #[test]
    fn ac_card3_small_known_cardinality_passes() {
        let catalog = make_catalog_with_known_cardinality(
            "ship_mode",
            "Carrier",
            20,
            Some(20),
        );
        let mqo = make_projection_mqo("ship_mode", "Carrier");
        let result = check_projection_cardinality(&mqo, &catalog, 1_000);
        assert!(result.is_ok(), "small known-cardinality level should pass; got: {result:?}");
    }

    // PRD AC-4: cardinality: None + no domain → cardinality_unknown fail-safe.
    #[test]
    fn ac_card4_no_cardinality_no_domain_is_unknown() {
        let catalog = make_catalog_with_known_cardinality(
            "store_dimension",
            "Store Id",
            0,    // no domain
            None, // no cardinality
        );
        let mqo = make_projection_mqo("store_dimension", "Store Id");
        let result = check_projection_cardinality(&mqo, &catalog, 10_000);
        assert!(result.is_err(), "no cardinality + no domain must fail safe");
        assert_eq!(result.unwrap_err().level, "cardinality_unknown");
    }

    // PRD AC-4 variant: cardinality: Some(0) + no domain → cardinality_unknown.
    #[test]
    fn ac_card4_zero_cardinality_no_domain_is_unknown() {
        let catalog = make_catalog_with_known_cardinality(
            "store_dimension",
            "Store Id",
            0,
            Some(0), // zero is treated as absent
        );
        let mqo = make_projection_mqo("store_dimension", "Store Id");
        let result = check_projection_cardinality(&mqo, &catalog, 10_000);
        assert!(result.is_err(), "zero cardinality + no domain must fail safe");
        assert_eq!(result.unwrap_err().level, "cardinality_unknown");
    }

    // PRD AC-5: cardinality: None but a domain present → falls back to domain.len().
    #[test]
    fn ac_card5_no_cardinality_falls_back_to_domain_len() {
        // 30-member domain, no cardinality field (old snapshot back-compat).
        let catalog = make_catalog_with_known_cardinality(
            "store_dimension",
            "State",
            30,
            None,
        );
        let mqo = make_projection_mqo("store_dimension", "State");
        // cap of 10,000 → should pass (30 < 10,000).
        let result = check_projection_cardinality(&mqo, &catalog, 10_000);
        assert!(result.is_ok(), "fall-back to domain.len() should pass for small domain; got: {result:?}");
    }

    // FR-5: selectivity is applied to the TRUE cardinality (not the truncated domain).
    // A range filter on a 10,436-cardinality level should multiply the true count,
    // not the 50-element domain.
    #[test]
    fn ac_card5_selectivity_uses_true_cardinality() {
        use mqo_spec::RangeBound;
        // Sold Calendar Week: 10,436 true cardinality, 50-member truncated domain.
        let catalog = make_catalog_with_known_cardinality(
            "sold_date_week_hierarchy",
            "Sold Calendar Week",
            50,
            Some(10_436),
        );
        let mut mqo = make_projection_mqo("sold_date_week_hierarchy", "Sold Calendar Week");
        // A tight 1-unit range: selectivity ≈ 2/10436 ≈ 0.019% → estimate ≈ 1.
        mqo.filters.push(Filter::Range {
            level: "sold_date_week_hierarchy.[Sold Calendar Week]".to_string(),
            lo: RangeBound::Number(100.0),
            hi: RangeBound::Number(101.0),
        });
        // With a cap of 1,000 this should pass (tight range on true total).
        let result = check_projection_cardinality(&mqo, &catalog, 1_000);
        assert!(
            result.is_ok(),
            "tight range on large-cardinality level should pass within cap; got: {result:?}"
        );
    }

    // ── cardinality-estimate-fix regression tests ───────────────────────────

    /// Build a catalog column for one level (realistic unique_name + bare level,
    /// true cardinality set).
    fn level_col(hierarchy: &str, level: &str, cardinality: u64) -> ColumnEntry {
        ColumnEntry {
            unique_name: format!("{hierarchy}.[{level}]"),
            label: level.to_string(),
            kind: "level".to_string(),
            hierarchy: Some(hierarchy.to_string()),
            level: Some(level.to_string()),
            cardinality: Some(cardinality),
            ..Default::default()
        }
    }

    fn projection_with_levels(model: &str, dims: &[(&str, &str)]) -> Mqo {
        Mqo {
            model: model.to_string(),
            measures: vec![],
            dimensions: dims
                .iter()
                .map(|(h, l)| LevelSelection {
                    hierarchy: (*h).to_string(),
                    level: (*l).to_string(),
                })
                .collect(),
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
            projection: true,
        }
    }

    // THE BUG: projecting several attributes of the SAME hierarchy must NOT
    // cross-multiply their cardinalities. `midway-stores` projects Store Name,
    // Store Manager, Store Floor Space — all store_dimension levels (~1000 each).
    // Old code: 1000 × 1000 × 1000 = 1e9 → wrongly declined. Fixed: the finest
    // level bounds the rows (max = 1000) → well under a 10k cap → passes.
    #[test]
    fn same_hierarchy_attributes_do_not_cross_multiply() {
        let catalog = CatalogSnapshot {
            columns: vec![
                level_col("store_dimension", "Store Name", 1002),
                level_col("store_dimension", "Store Manager", 900),
                level_col("store_dimension", "Store Floor Space", 1000),
            ],
            ..Default::default()
        };
        let mqo = projection_with_levels(
            "store_dimension",
            &[
                ("store_dimension", "Store Name"),
                ("store_dimension", "Store Manager"),
                ("store_dimension", "Store Floor Space"),
            ],
        );
        let result = check_projection_cardinality(&mqo, &catalog, 10_000);
        assert!(
            result.is_ok(),
            "same-hierarchy attribute projection must be bounded by the finest level (~1002), \
             not the product (~9e8); got: {result:?}"
        );
    }

    // GAP FIX (products-price-above-70): a Range filter on a NON-projected level
    // of the SAME hierarchy as the projection must reduce the estimate. Projecting
    // `Item Product Name` (206,021) WHERE `Product Current Price` > 70 was estimated
    // at the full unfiltered domain (206,021 > 50,000 cap → wrongly rejected), because
    // the Range arm only matched a filter on the *projected* level and the
    // cross-hierarchy path skipped it (same hierarchy). With the fix, a 1/10
    // selectivity is applied to the group base (206,021 → ~20,602 < 50,000 → passes).
    #[test]
    fn range_on_non_projected_level_of_same_hierarchy_reduces_estimate() {
        let catalog = CatalogSnapshot {
            columns: vec![
                level_col("product_dimension", "Item Product Name", 206_021),
                level_col("product_dimension", "Product Current Price", 0),
            ],
            ..Default::default()
        };
        let mut mqo = projection_with_levels(
            "tpcds_benchmark_model",
            &[("product_dimension", "Item Product Name")],
        );
        // Control: without the filter, the bare projection exceeds the 50k cap.
        assert!(
            check_projection_cardinality(&mqo, &catalog, 50_000).is_err(),
            "bare Item Product Name projection (206k) should exceed a 50k cap"
        );
        // With the price>70 Range filter on a non-projected level of the same hierarchy,
        // the 1/10 selectivity brings the estimate under the cap → passes.
        mqo.filters = vec![Filter::Range {
            level: "product_dimension.[Product Current Price]".to_string(),
            lo: RangeBound::Number(70.0),
            hi: RangeBound::Number(1_000_000.0),
        }];
        let result = check_projection_cardinality(&mqo, &catalog, 50_000);
        assert!(
            result.is_ok(),
            "a Range filter on a non-projected level of the same hierarchy must reduce \
             the estimate (206021/10 ≈ 20602 < 50000); got: {result:?}"
        );
    }

    // A member filter on ANY level of the hierarchy constrains the whole group:
    // `midway-stores` filters Store City='Midway', so all co-varying store
    // attributes collapse to that city's stores. Even with a tiny cap it passes.
    #[test]
    fn member_filter_constrains_whole_hierarchy_group() {
        let catalog = CatalogSnapshot {
            columns: vec![
                level_col("store_dimension", "Store Name", 1002),
                level_col("store_dimension", "Store Manager", 900),
                level_col("store_dimension", "Store Floor Space", 1000),
                level_col("store_dimension", "Store City", 250),
            ],
            ..Default::default()
        };
        let mut mqo = projection_with_levels(
            "store_dimension",
            &[
                ("store_dimension", "Store Name"),
                ("store_dimension", "Store Manager"),
                ("store_dimension", "Store Floor Space"),
            ],
        );
        // Filter Store City to a single city (the midway-stores shape).
        mqo.filters.push(Filter::MemberLevel {
            hierarchy: "store_dimension".to_string(),
            level: "store_dimension.[Store City]".to_string(),
            members: vec!["Midway".to_string()],
            exclude: false,
        });
        let result = check_projection_cardinality(&mqo, &catalog, 50);
        assert!(
            result.is_ok(),
            "a single-member filter on one level must constrain the whole co-varying group; got: {result:?}"
        );
    }

    // INDEPENDENT hierarchies DO legitimately cross-multiply — but under FR-4
    // (PRD-mqo-projection-handle-over-cap), when each individual hierarchy's
    // estimate is ≤ cap the product is treated as advisory (not a hard reject):
    // the runtime row_cap_tripped is the hard bound.  Hard rejection still fires
    // when a SINGLE hierarchy's own cardinality exceeds the cap.
    //
    // New behavior (FR-4): 200-member levels × 200 = 40k > 10k cap — but since
    // each factor (200) ≤ cap (10k), the estimate is advisory → proceeds.
    // Hard reject case: a level with 15,000 members > 10,000 cap → still rejects.
    #[test]
    fn independent_hierarchies_cross_multiply() {
        // FR-4: 200×200=40k with 10k cap — advisory because each factor ≤ cap.
        let catalog = CatalogSnapshot {
            columns: vec![
                level_col("store_dimension", "Store City", 200),
                level_col("product_dimension", "Product Brand Name", 200),
            ],
            ..Default::default()
        };
        let mqo = projection_with_levels(
            "tpcds_benchmark_model",
            &[
                ("store_dimension", "Store City"),
                ("product_dimension", "Product Brand Name"),
            ],
        );
        // Advisory under FR-4: each factor (200) ≤ cap (10k), product (40k) > cap →
        // cap at budget, proceed.
        let result = check_projection_cardinality(&mqo, &catalog, 10_000);
        assert!(
            result.is_ok(),
            "FR-4: cross-hierarchy product (200×200=40k) with each factor ≤ cap should be \
             advisory (proceeds); got: {result:?}"
        );
        // Hard reject: a level whose own cardinality exceeds the cap → still rejects.
        let catalog_large = CatalogSnapshot {
            columns: vec![
                level_col("store_dimension", "Store City", 15_000),
                level_col("product_dimension", "Product Brand Name", 200),
            ],
            ..Default::default()
        };
        let mqo_large = projection_with_levels(
            "tpcds_benchmark_model",
            &[
                ("store_dimension", "Store City"),
                ("product_dimension", "Product Brand Name"),
            ],
        );
        let result_large = check_projection_cardinality(&mqo_large, &catalog_large, 10_000);
        assert!(
            result_large.is_err(),
            "single hierarchy with 15k members > 10k cap must hard-reject; got: {result_large:?}"
        );
    }

    // ── PRD-mqo-projection-handle-over-cap new tests ─────────────────────────

    // FR-2/FR-3: estimate < budget → Ok(()) (proceeds, handled by large-result path)
    #[test]
    fn within_budget_proceeds_not_rejected() {
        // 8,000 members < cap 50,000 → must proceed
        let catalog = make_catalog_with_known_cardinality(
            "customer_dimension",
            "Customer First Name",
            0,
            Some(8_000),
        );
        let mqo = make_projection_mqo("customer_dimension", "Customer First Name");
        let result = check_projection_cardinality(&mqo, &catalog, 50_000);
        assert!(
            result.is_ok(),
            "estimate (8k) < budget (50k) must proceed; got: {result:?}"
        );
    }

    // FR-3: estimate > budget → Err(ProjectionTooLarge)
    #[test]
    fn over_budget_rejected() {
        // 75,000 members > cap 50,000 → must reject
        let catalog = make_catalog_with_known_cardinality(
            "customer_dimension",
            "Customer Id",
            0,
            Some(75_000),
        );
        let mqo = make_projection_mqo("customer_dimension", "Customer Id");
        let result = check_projection_cardinality(&mqo, &catalog, 50_000);
        assert!(result.is_err(), "estimate (75k) > budget (50k) must reject; got: {result:?}");
        let err = result.unwrap_err();
        assert!(
            err.estimate > 50_000,
            "error estimate must reflect over-budget count; got: {}",
            err.estimate
        );
    }

    // FR-4: product > budget but each individual hierarchy factor ≤ budget
    // → estimate is advisory (capped at budget), projection proceeds.
    // Models the `customers-ese` case: First Name (5,126) × Gender (2) = 10,252
    // which exceeds a 10k cap but is advisory at a 50k budget.
    #[test]
    fn cross_hierarchy_cap_at_budget() {
        let catalog = CatalogSnapshot {
            columns: vec![
                level_col("customer_name_dimension", "Customer First Name", 5_126),
                level_col("customer_demographics", "Gender", 2),
            ],
            ..Default::default()
        };
        let mqo = projection_with_levels(
            "tpcds_benchmark_model",
            &[
                ("customer_name_dimension", "Customer First Name"),
                ("customer_demographics", "Gender"),
            ],
        );
        // With a 10k cap, 5126 * 2 = 10252 > 10000, but each factor ≤ 10000 →
        // advisory: capped at 10000, passes.
        let result = check_projection_cardinality(&mqo, &catalog, 10_000);
        assert!(
            result.is_ok(),
            "cross-hierarchy product > cap but each factor ≤ cap must be advisory (proceeds); \
             customers-ese shape: 5126 × 2 = 10252 > 10000; got: {result:?}"
        );
    }

    // FR-5: Range/Member filter on a non-projected level applies cross-hierarchy
    // selectivity or demotes the estimate to advisory (within-budget).
    // Models the `products-price-above-70` case:
    //   project Item Product Name (domain 206,021) WHERE Product Current Price > 70
    //   guard estimates 206,021 (ignoring the filter) → projected_too_large.
    //   After fix: cross-hierarchy Range filter applies 1/10 selectivity →
    //   estimate ≈ 20,602 (or advisory) → within budget → proceeds.
    #[test]
    fn filter_on_non_projected_level_reduces_estimate_or_demotes() {
        let catalog = CatalogSnapshot {
            columns: vec![
                // Item Product Name: 206,021 total members (the full product-name domain).
                level_col("product_item_dimension", "Item Product Name", 206_021),
                // Product Current Price lives on a different hierarchy.
                level_col("product_price_dimension", "Product Current Price", 100),
            ],
            ..Default::default()
        };
        let mut mqo = projection_with_levels(
            "tpcds_benchmark_model",
            &[("product_item_dimension", "Item Product Name")],
        );
        // Range filter on the non-projected Product Current Price hierarchy.
        mqo.filters.push(Filter::Range {
            level: "product_price_dimension.[Product Current Price]".to_string(),
            lo: mqo_spec::RangeBound::Number(70.0),
            hi: mqo_spec::RangeBound::Number(10_000.0),
        });
        // Budget = 50,000. Without the fix, the guard estimates 206,021 and rejects.
        // With the fix, the cross-hierarchy Range applies 1/10 selectivity →
        // estimate ≈ 20,602 < 50,000 → proceeds.
        let result = check_projection_cardinality(&mqo, &catalog, 50_000);
        assert!(
            result.is_ok(),
            "Range filter on non-projected hierarchy must reduce estimate or demote to advisory; \
             products-price-above-70 shape: 206021 / 10 ≈ 20602 < 50000; got: {result:?}"
        );
    }
}
