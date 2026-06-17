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

// Pre-existing lint suppressions — do not remove without fixing the underlying code.
#![allow(
    clippy::doc_markdown, clippy::missing_errors_doc, clippy::missing_panics_doc,
    clippy::must_use_candidate, clippy::map_unwrap_or, clippy::manual_let_else,
    clippy::items_after_statements, clippy::too_many_lines, clippy::uninlined_format_args,
    clippy::cast_possible_truncation, clippy::cast_precision_loss, clippy::implicit_hasher,
    clippy::similar_names, clippy::redundant_closure_for_method_calls, clippy::map_clone,
    clippy::if_not_else, clippy::unnested_or_patterns, clippy::manual_range_patterns,
    clippy::explicit_auto_deref, clippy::doc_overindented_list_items,
    clippy::used_underscore_binding, clippy::absurd_extreme_comparisons, clippy::type_complexity
)]

use mqo_auth_bridge::{EngineError, LiveExecutor};
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
    /// Max number of `MDSCHEMA_MEMBERS` Discover requests in flight simultaneously.
    /// Default 16. Setting to 1 reproduces the old serial path exactly (FR-5).
    pub concurrency: usize,
}

/// Outcome counts for the startup summary (FR-4).
#[derive(Default)]
pub struct IngestSummary {
    pub measures_seen: usize,
    pub semi_additive_found: usize,
    pub levels_seen: usize,
    pub levels_mapped: usize,
    /// Levels with cardinality ≤ domain_cap that were eligible for domain capture.
    pub eligible: usize,
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
///
/// OLE DB numeric/decimal types map to `"decimal"` so the capture site
/// normalizes those members to the engine-comparable form (FR-1/FR-4):
///   4  = R4 (float32), 5 = R8 (float64), 6 = CY (currency)
///   131 = NUMERIC/DECIMAL (the type used for GMT-offset levels in AtScale)
/// Integer types (I1..I8, UI1..UI4) map to `"integer"`.
fn dbtype_to_value_type(dbtype: &str) -> &'static str {
    match dbtype {
        "2" | "3" | "16" | "17" | "18" | "19" | "20" | "21" => "integer",
        "4" | "5" | "6" | "131" => "decimal",
        "7" | "133" | "134" | "135" => "date",
        _ => "string", // 8/129/130 wstr/str/bstr + safe default
    }
}

/// Infer `value_type` from the actual member values (what a filter value is
/// compared against), preferred over `LEVEL_DBTYPE` which reflects the level's
/// KEY type — e.g. a "Product Brand Name" level keyed by an integer ID but whose
/// members are brand-name strings. Used whenever a domain was captured.
///
/// NOTE: when `LEVEL_DBTYPE` indicates decimal (types 4/5/6/131), this function
/// is called AFTER key normalization — so the sample values are already in the
/// engine-comparable form (e.g. `-5.00`). It recognizes that form as `"decimal"`.
fn infer_value_type_from_members(members: &[String]) -> &'static str {
    let is_int = |s: &str| {
        let t = s.strip_prefix('-').unwrap_or(s);
        !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())
    };
    let is_decimal = |s: &str| {
        // Matches: optional minus, digits, dot, digits (e.g. "-5.00", "3.14")
        let body = s.strip_prefix('-').unwrap_or(s);
        if let Some(dot) = body.find('.') {
            let (int_part, frac_part) = (&body[..dot], &body[dot + 1..]);
            !int_part.is_empty()
                && int_part.bytes().all(|b| b.is_ascii_digit())
                && !frac_part.is_empty()
                && frac_part.bytes().all(|b| b.is_ascii_digit())
        } else {
            false
        }
    };
    let is_date = |s: &str| {
        let b = s.as_bytes();
        s.len() == 10 && b[4] == b'-' && b[7] == b'-'
    };
    let sample: Vec<&String> = members.iter().take(50).collect();
    if sample.iter().all(|s| is_date(s)) {
        "date"
    } else if sample.iter().all(|s| is_decimal(s)) {
        "decimal"
    } else if sample.iter().all(|s| is_int(s)) {
        "integer"
    } else {
        "string"
    }
}

/// Select the engine-comparable member value for a given `value_type`.
///
/// For **decimal** levels (DBTYPE 4/5/6/131), the engine compares on the member
/// KEY, not the display caption (e.g. caption = `-5`, key = `-5.00`). We prefer
/// `MEMBER_KEY` when the rowset provides it; if it is absent or empty we fall
/// back to the caption unchanged (the caller may apply further normalization).
///
/// For **all other** value_types we return the caption unchanged — FR-3/FR-4:
/// string/integer/date domains must not be coerced, and the selection is driven
/// by `value_type` (from `LEVEL_DBTYPE`), never by a regex on the value itself.
fn select_member_value(
    caption: &str,
    key: Option<&str>,
    value_type: &str,
) -> String {
    match value_type {
        "decimal" => {
            // Prefer the MEMBER_KEY (engine-comparable form).
            let k = key.unwrap_or("").trim();
            if !k.is_empty() && k != "__NULL__" {
                k.to_string()
            } else {
                // Key absent: caption is already the best available value.
                caption.to_string()
            }
        }
        _ => caption.to_string(),
    }
}

/// True for genuine semi-additive aggregators (First/LastChild, First/LastNonEmpty).
/// 9 (AverageOfChildren) is deliberately excluded — AtScale applies it to
/// totals/calcs, not balances (verified against the live model 2026-06-12).
fn is_semi_additive_aggregator(agg: i64) -> bool {
    matches!(agg, 10 | 11 | 12 | 13)
}

/// Reduce a batch of per-level Discover results into the `domains` map.
///
/// Called from `ingest_live_metadata` after `discover_members_batch`. Extracted
/// so unit tests can exercise the reduction logic without a live executor.
/// Per-level errors increment `errored` and are skipped (FR-4, fail-open).
fn reduce_batch_results(
    results: Vec<((String, String), Result<Vec<BTreeMap<String, String>>, EngineError>)>,
    level_meta: &BTreeMap<(String, String), (&'static str, String, usize)>,
    domains: &mut BTreeMap<(String, String), Vec<String>>,
    summary: &mut IngestSummary,
) {
    for (key, result) in results {
        let level_vt = level_meta
            .get(&key)
            .map(|(vt, _, _)| *vt)
            .unwrap_or("string");
        match result {
            Ok(rows) => {
                let dom: Vec<String> = rows
                    .iter()
                    .filter_map(|r| {
                        let caption = r.get("MEMBER_CAPTION")?.as_str();
                        if caption == "__NULL__" || caption == "(All)" {
                            return None;
                        }
                        let key_val = r.get("MEMBER_KEY").map(String::as_str);
                        Some(select_member_value(caption, key_val, level_vt))
                    })
                    .filter(|m| m != "__NULL__" && m != "(All)")
                    .collect();
                if !dom.is_empty() {
                    domains.insert(key, dom);
                    summary.domains_captured += 1;
                }
            }
            Err(_) => summary.errored += 1,
        }
    }
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

    // Collect levels to fetch.
    // The cardinality gate (domain_cap) is the primary filter: only levels with
    // cardinality ≤ domain_cap are eligible. The max_levels cap is a secondary
    // wall-time guard (default effectively unlimited) so all low-cardinality
    // levels are captured regardless of their position in the schema.
    // Each entry: (catalog key, level_unique_name).
    let mut to_fetch: Vec<((String, String), String)> = Vec::new();
    for key in &catalog_levels {
        let Some((_, lun, card)) = level_meta.get(key) else { continue };
        if *card == 0 || *card > cfg.domain_cap {
            if *card > cfg.domain_cap {
                summary.over_cap += 1;
            }
            continue;
        }
        // Level is eligible (cardinality ≤ domain_cap).
        summary.eligible += 1;
        if to_fetch.len() >= cfg.max_levels {
            // max_levels cap hit — count remaining eligible levels but don't fetch.
            continue;
        }
        to_fetch.push((key.clone(), lun.clone()));
    }

    // Parallel fetch: one token, bounded concurrency (FR-1/FR-2).
    // Token is fetched once inside discover_members_batch (NFR-1).
    match ex.discover_members_batch(&to_fetch, xmla_catalog, cube, cfg.concurrency) {
        Ok(results) => {
            reduce_batch_results(results, &level_meta, &mut domains, &mut summary);
        }
        // Token fetch failed (the only error that aborts the whole batch).
        Err(_) => summary.errored += 1,
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

    // Completeness log: how many levels were eligible, captured, skipped-too-large, failed.
    // Emitted at info level so operators can see domain breadth at startup.
    let skipped_by_cap = summary.eligible.saturating_sub(summary.domains_captured + summary.errored);
    eprintln!(
        "mqo-mcp-server: domain coverage: {} eligible (card ≤ {}), {} captured, \
         {} skipped-too-large (card > {}), {} skipped-by-level-cap, {} errored",
        summary.eligible,
        cfg.domain_cap,
        summary.domains_captured,
        summary.over_cap,
        cfg.domain_cap,
        skipped_by_cap,
        summary.errored,
    );

    summary.wall_ms = start.elapsed().as_millis();
    summary
}

/// Build `{catalog_unique_name → LEVEL_CARDINALITY}` for the cache validity check.
///
/// Uses the same `(hierarchy_key, level_name)` matching as `ingest_live_metadata` to
/// translate MDSCHEMA's 3-part bracket `LEVEL_UNIQUE_NAME` into the catalog's
/// snake-cased convention (e.g. `store_dimension.[Store City]`), then joins against
/// the catalog columns. Only levels with a real, non-zero, in-range cardinality are
/// included — matching the storage condition in `ingest_live_metadata` so the diff
/// against `catalog_cache::cardinality_map` is apples-to-apples.
///
/// Returns an empty map on Discover failure (caller degrades to full re-ingest).
pub fn fresh_cardinality_map(
    ex: &crate::LiveExecutor,
    xmla_catalog: &str,
    cube: &str,
    catalog_columns: &serde_json::Value,
) -> std::collections::HashMap<String, usize> {
    let mut level_meta: std::collections::BTreeMap<(String, String), usize> =
        std::collections::BTreeMap::new();
    match ex.discover_mdschema("MDSCHEMA_LEVELS", xmla_catalog, cube, None) {
        Ok(rows) => {
            for r in &rows {
                let level_name = r.get("LEVEL_NAME").map(String::as_str).unwrap_or("");
                if level_name.is_empty() || level_name == "(All)" {
                    continue;
                }
                if r.get("LEVEL_NUMBER").map(String::as_str) == Some("0") {
                    continue;
                }
                let card = r.get("LEVEL_CARDINALITY")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0);
                if card == 0 || card >= usize::MAX {
                    continue;
                }
                let hier_key_v = r.get("HIERARCHY_UNIQUE_NAME")
                    .map(|h| hierarchy_key(h))
                    .unwrap_or_default();
                let dim_key_v = r.get("DIMENSION_UNIQUE_NAME")
                    .map(|d| snake(&strip_brackets(d)))
                    .unwrap_or_default();
                for key in [hier_key_v, dim_key_v] {
                    if !key.is_empty() {
                        level_meta.entry((key, level_name.to_string())).or_insert(card);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("mqo-mcp-server: catalog cache: MDSCHEMA_LEVELS failed: {e}");
            return std::collections::HashMap::new();
        }
    }
    // Join against catalog columns using their (hierarchy, level) fields.
    let mut out = std::collections::HashMap::new();
    if let Some(cols) = catalog_columns.as_array() {
        for col in cols {
            if col.get("kind").and_then(|v| v.as_str()) != Some("level") {
                continue;
            }
            let Some(un) = col.get("unique_name").and_then(|v| v.as_str()) else { continue };
            let hier = col.get("hierarchy").and_then(|v| v.as_str()).unwrap_or("");
            let level = col.get("level").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(&card) = level_meta.get(&(hier.to_string(), level.to_string())) {
                out.insert(un.to_string(), card);
            }
        }
    }
    out
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
        // FR-1/FR-4: DBTYPE 131 (NUMERIC/DECIMAL) and float types map to "decimal"
        // so the capture site can normalize to the engine-comparable form.
        assert_eq!(dbtype_to_value_type("131"), "decimal");
        assert_eq!(dbtype_to_value_type("4"), "decimal");  // R4 / float32
        assert_eq!(dbtype_to_value_type("5"), "decimal");  // R8 / float64
        assert_eq!(dbtype_to_value_type("6"), "decimal");  // CY / currency
        // Integers stay integer; text/unknown stay string.
        assert_eq!(dbtype_to_value_type("2"), "integer");
        assert_eq!(dbtype_to_value_type("20"), "integer");
        assert_eq!(dbtype_to_value_type("8"), "string");
    }

    #[test]
    fn value_type_inferred_from_members_not_key() {
        // A "Brand Name" level keyed by int but whose members are strings → string.
        assert_eq!(infer_value_type_from_members(&["Nike".into(), "Acme".into()]), "string");
        assert_eq!(infer_value_type_from_members(&["1".into(), "2".into()]), "integer");
        assert_eq!(infer_value_type_from_members(&["2001-01-15".into()]), "date");
        // Decimal form (engine-comparable after key normalization) → "decimal".
        assert_eq!(infer_value_type_from_members(&["-5.00".into(), "-6.00".into(), "-10.00".into()]), "decimal");
    }

    // ── FR-1/FR-4 unit tests: select_member_value normalization ──────────────

    /// AC for FR-1: a decimal-typed level with caption "-5" and key "-5.00"
    /// → domain stores the engine-comparable key "-5.00".
    #[test]
    fn decimal_level_uses_member_key() {
        assert_eq!(
            select_member_value("-5", Some("-5.00"), "decimal"),
            "-5.00",
            "decimal level: key should be preferred over caption"
        );
        assert_eq!(
            select_member_value("-6", Some("-6.00"), "decimal"),
            "-6.00"
        );
        // No key available: falls back to caption (best effort).
        assert_eq!(
            select_member_value("-5", None, "decimal"),
            "-5"
        );
        // Empty key: falls back to caption.
        assert_eq!(
            select_member_value("-5", Some(""), "decimal"),
            "-5"
        );
    }

    /// FR-3: string-typed level → caption unchanged regardless of value shape.
    #[test]
    fn string_level_caption_unchanged() {
        assert_eq!(
            select_member_value("CA", Some("42"), "string"),
            "CA",
            "string level: caption must be preserved, key ignored"
        );
        assert_eq!(
            select_member_value("WA", None, "string"),
            "WA"
        );
    }

    /// FR-4: text-typed level with numeric-looking values (zip/id) → NOT coerced.
    /// The value_type is "string" (from LEVEL_DBTYPE), so no decimal normalization occurs.
    #[test]
    fn text_level_numeric_looking_values_not_coerced() {
        // ZIP code: looks numeric but LEVEL_DBTYPE → "string", so stays as-is.
        assert_eq!(
            select_member_value("90210", Some("90210"), "string"),
            "90210",
            "zip-code level: must not be decimal-coerced"
        );
        // An ID that looks like a decimal: value_type from DBTYPE says "string".
        assert_eq!(
            select_member_value("1234.00", Some("1234.00"), "string"),
            "1234.00",
            "string-typed level with decimal-looking value: must not be decimal-coerced"
        );
    }

    /// FR-4: integer-typed level → caption unchanged (no decimal normalization).
    #[test]
    fn integer_level_caption_unchanged() {
        assert_eq!(
            select_member_value("5", Some("5.00"), "integer"),
            "5",
            "integer level: caption must not be coerced via decimal key"
        );
    }

    #[test]
    fn semi_additive_aggregator_excludes_avg_of_children() {
        assert!(is_semi_additive_aggregator(10)); // FirstChild
        assert!(is_semi_additive_aggregator(13)); // LastNonEmpty
        assert!(!is_semi_additive_aggregator(9)); // AverageOfChildren (AtScale artifact)
        assert!(!is_semi_additive_aggregator(1)); // SUM
        assert!(!is_semi_additive_aggregator(5)); // AVG
    }

    // ── AC-4: one failing level → error counted, others captured ─────────────

    fn make_row(caption: &str, key: Option<&str>) -> std::collections::BTreeMap<String, String> {
        let mut m = std::collections::BTreeMap::new();
        m.insert("MEMBER_CAPTION".into(), caption.into());
        if let Some(k) = key {
            m.insert("MEMBER_KEY".into(), k.into());
        }
        m
    }

    fn make_level_meta(key: (&str, &str), card: usize) -> BTreeMap<(String, String), (&'static str, String, usize)> {
        let mut m = BTreeMap::new();
        m.insert(
            (key.0.into(), key.1.into()),
            ("string", format!("[{}].[{}].[{}]", key.0, key.0, key.1), card),
        );
        m
    }

    /// AC-4: one level errors → errored count is 1, other levels still captured.
    #[test]
    fn ac4_one_failing_level_counted_others_captured() {
        use super::EngineError;

        let key_ok = (("store_dimension".to_string(), "Store State".to_string()));
        let key_fail = (("ship_mode".to_string(), "Mode".to_string()));

        let mut level_meta: BTreeMap<(String, String), (&'static str, String, usize)> = BTreeMap::new();
        level_meta.insert(key_ok.clone(), ("string", "[Store].[Store State]".into(), 50));
        level_meta.insert(key_fail.clone(), ("string", "[Ship].[Mode]".into(), 10));

        let results: Vec<((String, String), Result<_, EngineError>)> = vec![
            (key_ok.clone(), Ok(vec![make_row("CA", None), make_row("WA", None)])),
            (key_fail.clone(), Err(EngineError::QueryError { reason: "simulated failure".into() })),
        ];

        let mut domains: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        let mut summary = IngestSummary::default();

        reduce_batch_results(results, &level_meta, &mut domains, &mut summary);

        assert_eq!(summary.errored, 1, "one error should be counted");
        assert_eq!(summary.domains_captured, 1, "successful level should be captured");
        assert!(domains.contains_key(&key_ok), "ok level domain should be present");
        assert!(!domains.contains_key(&key_fail), "failed level should be absent");
        assert_eq!(domains[&key_ok], vec!["CA", "WA"]);
    }

    /// AC-5: concurrency=1 vs concurrency=N produce same result (same BTreeMap
    /// key structure). Tested here by verifying reduce_batch_results is
    /// order-independent — same inputs in different order yield same domains.
    #[test]
    fn ac5_reduce_is_order_independent() {
        use super::EngineError;

        let key_a = ("dim_a".to_string(), "Level A".to_string());
        let key_b = ("dim_b".to_string(), "Level B".to_string());
        let key_c = ("dim_c".to_string(), "Level C".to_string());

        let mut level_meta: BTreeMap<(String, String), (&'static str, String, usize)> = BTreeMap::new();
        level_meta.insert(key_a.clone(), ("string", "[A].[A]".into(), 5));
        level_meta.insert(key_b.clone(), ("string", "[B].[B]".into(), 5));
        level_meta.insert(key_c.clone(), ("string", "[C].[C]".into(), 5));

        // Order 1: A, B, C
        let results1: Vec<((String, String), Result<_, EngineError>)> = vec![
            (key_a.clone(), Ok(vec![make_row("a1", None), make_row("a2", None)])),
            (key_b.clone(), Ok(vec![make_row("b1", None)])),
            (key_c.clone(), Ok(vec![make_row("c1", None), make_row("c2", None), make_row("c3", None)])),
        ];
        // Order 2: C, A, B (simulates different completion order under concurrency)
        let results2: Vec<((String, String), Result<_, EngineError>)> = vec![
            (key_c.clone(), Ok(vec![make_row("c1", None), make_row("c2", None), make_row("c3", None)])),
            (key_a.clone(), Ok(vec![make_row("a1", None), make_row("a2", None)])),
            (key_b.clone(), Ok(vec![make_row("b1", None)])),
        ];

        let mut domains1: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        let mut summary1 = IngestSummary::default();
        reduce_batch_results(results1, &level_meta, &mut domains1, &mut summary1);

        let mut domains2: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
        let mut summary2 = IngestSummary::default();
        reduce_batch_results(results2, &level_meta, &mut domains2, &mut summary2);

        assert_eq!(domains1, domains2, "BTreeMap result must be order-independent (FR-3/AC-5)");
        assert_eq!(summary1.domains_captured, summary2.domains_captured);
        assert_eq!(summary1.errored, summary2.errored);
    }
}
