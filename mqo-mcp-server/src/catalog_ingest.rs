//! Live catalog metadata ingestion via XMLA MDSCHEMA (PRD-mqo-live-catalog-ingestion v0.2).
//!
//! When the server runs in live mode and `--capture-live-domains` is set, this
//! module pulls catalog metadata from the cluster's **MDSCHEMA Discover** rowsets
//! and layers it onto the in-memory catalog's columns:
//!   * `MDSCHEMA_MEASURES` → `semi_additive` (aggregator ∈ {10,11,12,13};
//!     First/LastChild, First/LastNonEmpty — NOT 9 AverageOfChildren, which AtScale
//!     applies to totals/calcs).
//!   * `MDSCHEMA_LEVELS`   → `value_type` (`LEVEL_DBTYPE`) + cardinality gating
//!     (`LEVEL_CARDINALITY`).
//!   * `MDSCHEMA_MEMBERS`  → `domain` (member captions), fetched ONLY for levels
//!     with `LEVEL_CARDINALITY ≤ cap`.
//!
//! This supersedes the v0.21.0 MQO domain probe: no measure-pairing, no query
//! execution, cardinality-gated, types from metadata. The validator filter-level
//! guard and binder member-grounding check then run on live data, not a fixture.
//!
//! Name mapping (PRD OQ-5): MDSCHEMA is MDX form (`[Store Dimension]`, captions);
//! the catalog is snake_case (`store_dimension`, level label `Store State Name`).
//! We map `snake(DIMENSION caption) == catalog hierarchy` and
//! `LEVEL_NAME == catalog level label`; levels that don't map are counted + skipped.

use mqo_auth_bridge::LiveExecutor;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Instant;

/// Operator-tunable ingestion bounds.
pub struct IngestConfig {
    /// Max distinct members enumerated per level; above this, `value_type` is
    /// still set but the domain is not fetched (descriptor only).
    pub domain_cap: usize,
    /// Max number of `MDSCHEMA_MEMBERS` fetches (bounds startup wall-time).
    pub max_levels: usize,
}

/// Outcome counts for the startup summary (FR-4).
#[derive(Default)]
pub struct IngestSummary {
    pub measures_seen: usize,
    pub semi_additive_found: usize,
    pub levels_seen: usize,
    pub levels_mapped: usize,
    pub domains_captured: usize,
    pub over_cap: usize,
    pub errored: usize,
    pub wall_ms: u128,
}

/// `[Store Dimension]` → `Store Dimension`; `[A].[B]` → `A` (first segment).
fn strip_brackets(unique_name: &str) -> String {
    let first = unique_name.split("].[").next().unwrap_or(unique_name);
    first.trim_start_matches('[').trim_end_matches(']').to_string()
}

/// The catalog hierarchy key from a `HIERARCHY_UNIQUE_NAME` — `snake` of its LAST
/// bracket segment (the hierarchy caption). This matches the server catalog's
/// hierarchy naming for ALL hierarchies, including week hierarchies that share a
/// dimension (e.g. `[Sold Date Dimensions].[Sold Date Week Hierarchy]` →
/// `sold_date_week_hierarchy`), where keying on the DIMENSION caption fails (OQ-5).
fn hierarchy_key(hierarchy_unique_name: &str) -> String {
    let last = hierarchy_unique_name
        .rsplit("].[")
        .next()
        .unwrap_or(hierarchy_unique_name);
    snake(last.trim_start_matches('[').trim_end_matches(']'))
}

/// `Store Dimension` → `store_dimension`; `Avg … 1998-1999` → `avg_…_1998_1999`.
/// Lowercases and replaces every run of non-alphanumeric chars with a single
/// underscore (the catalog's snake convention, incl. hyphens/punctuation).
fn snake(caption: &str) -> String {
    let mut out = String::with_capacity(caption.len());
    let mut prev_us = false;
    for c in caption.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_us = false;
        } else if !prev_us {
            out.push('_');
            prev_us = true;
        }
    }
    out.trim_matches('_').to_string()
}

/// Map an OLE DB `LEVEL_DBTYPE` to the validator's `value_type`.
fn dbtype_to_value_type(dbtype: &str) -> &'static str {
    match dbtype {
        "2" | "3" | "16" | "17" | "18" | "19" | "20" | "21" => "integer",
        "7" | "133" | "134" | "135" => "date",
        _ => "string", // 8/129/130 wstr/str/bstr + safe default for numeric/decimal
    }
}

/// Infer `value_type` from the actual member captions (what a filter value is
/// compared against), preferred over `LEVEL_DBTYPE` which reflects the level's
/// KEY type — e.g. a "Product Brand Name" level keyed by an integer ID but whose
/// members are brand-name strings. Used whenever a domain was captured.
fn infer_value_type_from_members(members: &[String]) -> &'static str {
    let is_int = |s: &str| {
        let t = s.strip_prefix('-').unwrap_or(s);
        !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())
    };
    let is_date = |s: &str| {
        let b = s.as_bytes();
        s.len() == 10 && b[4] == b'-' && b[7] == b'-'
    };
    let sample: Vec<&String> = members.iter().take(50).collect();
    if sample.iter().all(|s| is_date(s)) {
        "date"
    } else if sample.iter().all(|s| is_int(s)) {
        "integer"
    } else {
        "string"
    }
}

/// True for genuine semi-additive aggregators (First/LastChild, First/LastNonEmpty).
/// 9 (AverageOfChildren) is deliberately excluded — AtScale applies it to
/// totals/calcs, not balances (verified against the live model 2026-06-12).
fn is_semi_additive_aggregator(agg: i64) -> bool {
    matches!(agg, 10 | 11 | 12 | 13)
}

/// Probe the live cluster via MDSCHEMA and layer `semi_additive` / `value_type` /
/// `domain` onto `catalog`'s columns in place. Fail-open: a failed Discover is
/// counted and skipped, never propagated.
pub fn ingest_live_metadata(
    catalog: &mut Value,
    ex: &LiveExecutor,
    xmla_catalog: &str,
    cube: &str,
    cfg: &IngestConfig,
) -> IngestSummary {
    let start = Instant::now();
    let mut summary = IngestSummary::default();

    // ── 1. Measures → semi_additive (by caption) ──────────────────────────
    let mut semi_by_caption: BTreeMap<String, bool> = BTreeMap::new();
    match ex.discover_mdschema("MDSCHEMA_MEASURES", xmla_catalog, cube, None) {
        Ok(rows) => {
            summary.measures_seen = rows.len();
            for r in &rows {
                let Some(name) = r.get("MEASURE_NAME") else { continue };
                let agg = r
                    .get("MEASURE_AGGREGATOR")
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(-1);
                let sa = is_semi_additive_aggregator(agg);
                if sa {
                    summary.semi_additive_found += 1;
                }
                semi_by_caption.insert(name.clone(), sa);
            }
        }
        Err(_) => summary.errored += 1,
    }

    // ── 2. Levels → value_type + cardinality (by hierarchy+level) ─────────
    // (hierarchy, level) -> (value_type, level_unique_name, cardinality)
    let mut level_meta: BTreeMap<(String, String), (&'static str, String, usize)> = BTreeMap::new();
    match ex.discover_mdschema("MDSCHEMA_LEVELS", xmla_catalog, cube, None) {
        Ok(rows) => {
            for r in &rows {
                summary.levels_seen += 1;
                let level_name = r.get("LEVEL_NAME").map(String::as_str).unwrap_or("");
                if level_name.is_empty() || level_name == "(All)" {
                    continue; // skip the (All) level
                }
                if r.get("LEVEL_NUMBER").map(String::as_str) == Some("0") {
                    continue;
                }
                // AtScale exposes most levels as ATTRIBUTE hierarchies named after
                // the attribute (`[Customer Address].[Customer State Name]`), while
                // the catalog groups them under the DIMENSION (`customer_address`);
                // user hierarchies (week) are grouped under the hierarchy caption.
                // Neither key alone matches — so register under BOTH the dimension
                // key and the hierarchy-caption key (OQ-5). The merge looks up the
                // catalog's actual hierarchy and finds whichever applies.
                let hier_key_v = r
                    .get("HIERARCHY_UNIQUE_NAME")
                    .map(|h| hierarchy_key(h))
                    .unwrap_or_default();
                let dim_key_v = r
                    .get("DIMENSION_UNIQUE_NAME")
                    .map(|d| snake(&strip_brackets(d)))
                    .unwrap_or_default();
                let vt = dbtype_to_value_type(r.get("LEVEL_DBTYPE").map(String::as_str).unwrap_or(""));
                let lun = r.get("LEVEL_UNIQUE_NAME").cloned().unwrap_or_default();
                let card = r
                    .get("LEVEL_CARDINALITY")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(usize::MAX);
                for key in [hier_key_v, dim_key_v] {
                    if !key.is_empty() {
                        level_meta
                            .entry((key, level_name.to_string()))
                            .or_insert_with(|| (vt, lun.clone(), card));
                    }
                }
            }
        }
        Err(_) => summary.errored += 1,
    }

    // ── 3. Domains via MDSCHEMA_MEMBERS for low-card mapped levels ────────
    // (hierarchy, level) -> domain
    let mut domains: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    // Which (hierarchy, level) keys actually exist in the catalog?
    let catalog_levels: Vec<(String, String)> = catalog
        .get("columns")
        .and_then(Value::as_array)
        .map(|cols| {
            cols.iter()
                .filter(|c| c.get("kind").and_then(Value::as_str) == Some("level"))
                .filter_map(|c| {
                    Some((
                        c.get("hierarchy").and_then(Value::as_str)?.to_string(),
                        c.get("level").and_then(Value::as_str)?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut fetches = 0usize;
    for key in &catalog_levels {
        let Some((_, lun, card)) = level_meta.get(key) else { continue };
        if *card == 0 || *card > cfg.domain_cap {
            if *card > cfg.domain_cap {
                summary.over_cap += 1;
            }
            continue;
        }
        if fetches >= cfg.max_levels {
            break;
        }
        fetches += 1;
        match ex.discover_mdschema("MDSCHEMA_MEMBERS", xmla_catalog, cube, Some(lun)) {
            Ok(rows) => {
                let dom: Vec<String> = rows
                    .iter()
                    .filter_map(|r| r.get("MEMBER_CAPTION").cloned())
                    .filter(|m| m != "__NULL__" && m != "(All)")
                    .collect();
                if !dom.is_empty() {
                    domains.insert(key.clone(), dom);
                    summary.domains_captured += 1;
                }
            }
            Err(_) => summary.errored += 1,
        }
    }

    // ── 4. Merge onto catalog columns ─────────────────────────────────────
    if let Some(arr) = catalog.get_mut("columns").and_then(Value::as_array_mut) {
        for col in arr.iter_mut() {
            match col.get("kind").and_then(Value::as_str) {
                Some("measure") => {
                    let label = col.get("label").and_then(Value::as_str).map(str::to_string);
                    if let Some(l) = label {
                        if semi_by_caption.get(&l).copied().unwrap_or(false) {
                            if let Some(o) = col.as_object_mut() {
                                o.insert("semi_additive".into(), json!({ "trigger_hierarchies": [] }));
                            }
                        }
                    }
                }
                Some("level") => {
                    let h = col.get("hierarchy").and_then(Value::as_str).unwrap_or_default().to_string();
                    let l = col.get("level").and_then(Value::as_str).unwrap_or_default().to_string();
                    let key = (h, l);
                    if let Some((vt, _, card)) = level_meta.get(&key) {
                        summary.levels_mapped += 1;
                        let dom = domains.get(&key);
                        // Prefer value_type inferred from the captured members
                        // (the caption type) over LEVEL_DBTYPE (the key type).
                        let value_type = dom.map_or(*vt, |d| infer_value_type_from_members(d));
                        if let Some(o) = col.as_object_mut() {
                            o.insert("value_type".into(), json!(value_type));
                            if let Some(d) = dom {
                                o.insert("domain".into(), json!(d));
                            }
                            // Persist the true LEVEL_CARDINALITY onto the column so
                            // the projection guard can use it instead of domain.len()
                            // (which is capped at domain_cap and thus truncated for
                            // high-cardinality levels like Sold Calendar Week).
                            // card == usize::MAX means the cluster reported no
                            // LEVEL_CARDINALITY (defaulted); skip those.
                            if *card != 0 && *card != usize::MAX {
                                o.insert("cardinality".into(), json!(*card as u64));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    summary.wall_ms = start.elapsed().as_millis();
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_mapping() {
        assert_eq!(strip_brackets("[Store Dimension]"), "Store Dimension");
        assert_eq!(snake("Store Dimension"), "store_dimension");
        assert_eq!(snake("Ship Mode"), "ship_mode");
        // hierarchy_key uses the LAST caption segment — week hierarchies map.
        assert_eq!(hierarchy_key("[Store Dimension].[Store Dimension]"), "store_dimension");
        assert_eq!(
            hierarchy_key("[Sold Date Dimensions].[Sold Date Week Hierarchy]"),
            "sold_date_week_hierarchy"
        );
        assert_eq!(hierarchy_key("[Ship Mode]"), "ship_mode");
    }

    #[test]
    fn dbtype_mapping() {
        assert_eq!(dbtype_to_value_type("130"), "string");
        assert_eq!(dbtype_to_value_type("3"), "integer");
        assert_eq!(dbtype_to_value_type("7"), "date");
        assert_eq!(dbtype_to_value_type("131"), "string"); // numeric → safe default
    }

    #[test]
    fn value_type_inferred_from_members_not_key() {
        // A "Brand Name" level keyed by int but whose members are strings → string.
        assert_eq!(infer_value_type_from_members(&["Nike".into(), "Acme".into()]), "string");
        assert_eq!(infer_value_type_from_members(&["1".into(), "2".into()]), "integer");
        assert_eq!(infer_value_type_from_members(&["2001-01-15".into()]), "date");
    }

    #[test]
    fn semi_additive_aggregator_excludes_avg_of_children() {
        assert!(is_semi_additive_aggregator(10)); // FirstChild
        assert!(is_semi_additive_aggregator(13)); // LastNonEmpty
        assert!(!is_semi_additive_aggregator(9)); // AverageOfChildren (AtScale artifact)
        assert!(!is_semi_additive_aggregator(1)); // SUM
        assert!(!is_semi_additive_aggregator(5)); // AVG
    }
}
