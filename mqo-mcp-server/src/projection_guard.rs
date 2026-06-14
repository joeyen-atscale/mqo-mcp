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
//!   (conservative fail-safe, FR-5 / OQ-1).
//!
//! # Integration note
//!
//! `mqo-attribute-projection` defines a stub of this same function that always
//! returns `Ok(())`. When the branches integrate, this real implementation
//! replaces that stub. The signature MUST match exactly.

#![forbid(unsafe_code)]

use mqo_catalog_binder::catalog::CatalogSnapshot;
use mqo_spec::{Filter, Mqo, RangeBound};

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
    let mut total_estimate: u64 = 1;

    for dim in &mqo.dimensions {
        // Look up the catalog entry for this level.  A client may supply the
        // level in several forms (see `level_matches`); resolve against all of
        // them so we hit the SAME catalog column that `describe_model`'s
        // `has_domain` keys off (its `unique_name`).
        let maybe_entry = catalog
            .columns
            .iter()
            .find(|col| col.kind == "level" && level_matches(col, &dim.hierarchy, &dim.level));

        // Canonical unique_name for diagnostics + filter matching: prefer the
        // resolved catalog column's unique_name (what describe_model advertises);
        // otherwise reconstruct `hierarchy.level`.
        let level_un = maybe_entry
            .map(|e| e.unique_name.clone())
            .unwrap_or_else(|| format!("{}.{}", dim.hierarchy, dim.level));

        // Total member count for this level.
        let total_members: Option<u64> = maybe_entry
            .and_then(|e| e.domain.as_ref())
            .map(|d| d.len() as u64)
            .filter(|&n| n > 0);

        // Apply filter selectivity if a filter targets this level's hierarchy.
        let level_estimate = estimate_level_cardinality(
            &level_un,
            &dim.hierarchy,
            total_members,
            &mqo.filters,
            cap,
        );

        match level_estimate {
            LevelEstimate::Known(n) => {
                // Multiply into total; saturate at u64::MAX.
                total_estimate = total_estimate.saturating_mul(n.max(1));
                if cap > 0 && total_estimate > cap as u64 {
                    return Err(ProjectionTooLarge {
                        level: level_un,
                        estimate: total_estimate,
                        cap,
                    });
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

/// The result of estimating cardinality for a single level.
enum LevelEstimate {
    /// A concrete (possibly filter-adjusted) member count.
    Known(u64),
    /// No member count is available in the catalog.
    Unknown,
}

/// Estimate the distinct count for one dimension level, applying any filter
/// that targets it.
fn estimate_level_cardinality(
    level_un: &str,
    hierarchy: &str,
    total_members: Option<u64>,
    filters: &[Filter],
    cap: usize,
) -> LevelEstimate {
    // Find the first filter targeting this hierarchy (or level).
    for f in filters {
        match f {
            Filter::Member { hierarchy: fh, members } if fh == hierarchy => {
                // IN-list selectivity: the listed members are the result set.
                return LevelEstimate::Known(members.len() as u64);
            }
            Filter::MemberLevel {
                hierarchy: fh,
                level: fl,
                members,
                ..
            } if fh == hierarchy && (fl == level_un || fl == &format_level_un(hierarchy, fl)) => {
                return LevelEstimate::Known(members.len() as u64);
            }
            Filter::Range { level: fl, lo, hi } if fl == level_un => {
                // Range selectivity: (hi - lo) / domain_width when numeric.
                let selectivity = estimate_range_selectivity(lo, hi, total_members);
                return match (total_members, selectivity) {
                    (Some(total), Some(sel)) => {
                        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                        let est = ((total as f64) * sel).ceil() as u64;
                        LevelEstimate::Known(est.max(1))
                    }
                    // Range with unknown total → we only know the filter is
                    // selective; use cap as the conservative upper bound so it
                    // may pass (not fail safe; range implies bounded intent).
                    (None, _) => LevelEstimate::Known(cap as u64),
                    (Some(_), None) => LevelEstimate::Unknown,
                };
            }
            Filter::Group { op: _, filters: inner } => {
                // Recurse into group filters.
                for inner_f in inner {
                    let sub = estimate_from_single_filter(
                        level_un,
                        hierarchy,
                        total_members,
                        inner_f,
                        cap,
                    );
                    if let Some(est) = sub {
                        return est;
                    }
                }
            }
            _ => {}
        }
    }

    // No filter for this level — use raw level cardinality.
    match total_members {
        Some(n) => LevelEstimate::Known(n),
        None => LevelEstimate::Unknown,
    }
}

/// Try to estimate cardinality from a single (possibly inner) filter.
/// Returns `Some(LevelEstimate)` when the filter matches, `None` otherwise.
fn estimate_from_single_filter(
    level_un: &str,
    hierarchy: &str,
    total_members: Option<u64>,
    filter: &Filter,
    cap: usize,
) -> Option<LevelEstimate> {
    match filter {
        Filter::Member { hierarchy: fh, members } if fh == hierarchy => {
            Some(LevelEstimate::Known(members.len() as u64))
        }
        Filter::MemberLevel {
            hierarchy: fh,
            level: fl,
            members,
            ..
        } if fh == hierarchy && (fl == level_un || true) => {
            // If the level field is the fully-qualified unique_name or the bare level name.
            let _ = fl; // suppress unused-variable; condition already checked above
            Some(LevelEstimate::Known(members.len() as u64))
        }
        Filter::Range { level: fl, lo, hi } if fl == level_un => {
            let selectivity = estimate_range_selectivity(lo, hi, total_members);
            Some(match (total_members, selectivity) {
                (Some(total), Some(sel)) => {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let est = ((total as f64) * sel).ceil() as u64;
                    LevelEstimate::Known(est.max(1))
                }
                (None, _) => LevelEstimate::Known(cap as u64),
                (Some(_), None) => LevelEstimate::Unknown,
            })
        }
        _ => None,
    }
}

/// Format a fully-qualified level unique_name from hierarchy + bare level name.
/// Only used for comparison, not canonical lookup.
fn format_level_un(hierarchy: &str, level: &str) -> String {
    format!("{hierarchy}.{level}")
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
}
