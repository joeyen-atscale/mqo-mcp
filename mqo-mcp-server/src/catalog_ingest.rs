//! Live catalog domain ingestion (PRD-mqo-live-catalog-ingestion, slice 1).
//!
//! When the server runs in live mode (`ServerEngine::Live`) and
//! `--capture-live-domains` is set, this module probes the cluster for each
//! dimension level's enumerated member domain — one bounded `measure + level`
//! query per level through the existing [`crate::pipeline::run`] path — and
//! layers `value_type` / `domain` / `expected_key_shape` onto the in-memory
//! catalog's level columns. This is the live-data source for the validator
//! filter-level guard and the binder member-grounding check, replacing the
//! hand-edited fixture domains.
//!
//! Scope of this slice (per the PRD): the bounded-DISTINCT **domain** probe
//! (FR-2), capped (FR-2/NFR-2), fail-open per level (FR-3), with a startup
//! summary (FR-4). Full live **column/`semi_additive`** ingestion (FR-1, gated
//! on PRD OQ-1) and disk caching/refresh (FR-5) are follow-on.

use crate::pipeline::{self, ToolPaths};
use crate::probe::BackendCapabilities;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

/// Operator-tunable ingestion bounds.
pub struct IngestConfig {
    /// Max distinct members enumerated per level; above this, carry a descriptor only.
    pub domain_cap: usize,
    /// Max number of levels probed (bounds startup wall-time on wide models).
    pub max_levels: usize,
    /// How many catalog measures to try when finding one that reaches a level's fact.
    pub probe_measure_attempts: usize,
}

/// Outcome counts for the startup summary (FR-4).
#[derive(Default)]
pub struct IngestSummary {
    pub levels_seen: usize,
    pub domains_captured: usize,
    pub over_cap: usize,
    pub errored: usize,
    pub wall_ms: u128,
}

/// Extract the dimension-column value from a result row. XMLA-mangled keys: a
/// MEASURE is `[Name]` → `_x005b_…`; a level is table-qualified
/// `atscale_catalogs[Name]` → does NOT start with `_x005b_`. Integer-valued
/// floats (e.g. a `5159.0` week sequence) render as `"5159"`.
fn dim_value(row: &Value) -> Option<String> {
    let obj = row.as_object()?;
    for (k, v) in obj {
        if k.starts_with("_x005b_") {
            continue; // measure column
        }
        return match v {
            Value::String(s) if !s.is_empty() => Some(s.clone()),
            Value::Number(n) => {
                if let Some(f) = n.as_f64() {
                    if f.fract() == 0.0 {
                        return Some((f as i64).to_string());
                    }
                }
                Some(n.to_string())
            }
            _ => None,
        };
    }
    None
}

fn infer_value_type(samples: &[String]) -> &'static str {
    let is_int = |s: &str| {
        let t = s.strip_prefix('-').unwrap_or(s);
        !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())
    };
    let is_date = |s: &str| {
        let b = s.as_bytes();
        s.len() == 10 && b[4] == b'-' && b[7] == b'-'
    };
    if samples.iter().all(|s| is_date(s)) {
        "date"
    } else if samples.iter().all(|s| is_int(s)) {
        "integer"
    } else {
        "string"
    }
}

/// Probe the live cluster for level member domains and layer them onto
/// `catalog`'s level columns in place. Fail-open: any per-level error is counted
/// and skipped — never propagated. Returns a summary for the startup log.
#[allow(clippy::too_many_arguments)]
pub fn capture_domains<S: std::hash::BuildHasher>(
    catalog: &mut Value,
    stats: &Value,
    tools: &ToolPaths,
    row_threshold: u64,
    engine: &crate::mcp::ServerEngine,
    backend_override: Option<&str>,
    capabilities: &BackendCapabilities,
    coords: &HashMap<String, (String, String), S>,
    model: &str,
    cfg: &IngestConfig,
) -> IngestSummary {
    let start = Instant::now();
    let mut summary = IngestSummary::default();

    let Some(cols) = catalog.get("columns").and_then(Value::as_array).cloned() else {
        return summary;
    };

    let measures: Vec<String> = cols
        .iter()
        .filter(|c| c.get("kind").and_then(Value::as_str) == Some("measure"))
        .filter_map(|c| c.get("unique_name").and_then(Value::as_str).map(String::from))
        .collect();
    let levels: Vec<(String, String)> = cols
        .iter()
        .filter(|c| c.get("kind").and_then(Value::as_str) == Some("level"))
        .filter_map(|c| {
            Some((
                c.get("hierarchy").and_then(Value::as_str)?.to_string(),
                c.get("level").and_then(Value::as_str)?.to_string(),
            ))
        })
        .collect();

    let candidates: Vec<String> = measures.iter().take(cfg.probe_measure_attempts).cloned().collect();

    // working measure per hierarchy (so we don't re-search per level)
    let mut hier_measure: BTreeMap<String, String> = BTreeMap::new();
    // (hierarchy, level) -> (value_type, Option<domain>)
    let mut results: Vec<(String, String, &'static str, Option<Vec<String>>)> = Vec::new();

    for (hier, level) in levels.iter().take(cfg.max_levels) {
        summary.levels_seen += 1;
        let try_measures: Vec<String> = hier_measure
            .get(hier)
            .map(|m| vec![m.clone()])
            .unwrap_or_else(|| candidates.clone());

        let mut captured = false;
        for meas in &try_measures {
            let mqo = json!({
                "model": model,
                "measures": [{ "unique_name": meas }],
                "dimensions": [{ "hierarchy": hier, "level": level }],
                "filters": [],
                "time_intelligence": [],
                "non_empty": true,
                "limit": (cfg.domain_cap as u64) + 1
            });
            // enriched_catalog_json=None: probe queries are single-fact, no cross-fact check needed.
            let out = pipeline::run(
                &mqo,
                &*catalog,
                stats,
                tools,
                row_threshold,
                engine,
                backend_override,
                capabilities,
                None,
                coords,
            );
            match out {
                Ok(po) => {
                    let mut dom: Vec<String> = po.rows.iter().filter_map(dim_value).collect();
                    dom.sort();
                    dom.dedup();
                    if dom.is_empty() {
                        continue; // wrong fact for this measure → try next
                    }
                    hier_measure.entry(hier.clone()).or_insert_with(|| meas.clone());
                    let vt = infer_value_type(&dom);
                    if dom.len() <= cfg.domain_cap {
                        results.push((hier.clone(), level.clone(), vt, Some(dom)));
                        summary.domains_captured += 1;
                    } else {
                        results.push((hier.clone(), level.clone(), vt, None));
                        summary.over_cap += 1;
                    }
                    captured = true;
                    break;
                }
                Err(_) => continue, // fail-open: try the next candidate measure
            }
        }
        if !captured {
            summary.errored += 1;
        }
    }

    // Merge captured metadata back onto the level columns.
    if let Some(arr) = catalog.get_mut("columns").and_then(Value::as_array_mut) {
        for col in arr.iter_mut() {
            if col.get("kind").and_then(Value::as_str) != Some("level") {
                continue;
            }
            let h = col.get("hierarchy").and_then(Value::as_str).unwrap_or_default().to_string();
            let l = col.get("level").and_then(Value::as_str).unwrap_or_default().to_string();
            if let Some((_, _, vt, dom)) = results.iter().find(|(hh, ll, _, _)| *hh == h && *ll == l) {
                if let Some(obj) = col.as_object_mut() {
                    obj.insert("value_type".into(), json!(vt));
                    match dom {
                        Some(d) => {
                            obj.insert("domain".into(), json!(d));
                        }
                        None => {
                            obj.insert(
                                "expected_key_shape".into(),
                                json!(format!(
                                    "{vt} member key; >{} distinct values (high-cardinality)",
                                    cfg.domain_cap
                                )),
                            );
                        }
                    }
                }
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
    fn dim_value_picks_dimension_not_measure() {
        let row = json!({
            "_x005b_Total_x0020_Store_x0020_Sales_x005d_": 1467409889.7,
            "atscale_catalogs_x005b_Store_x0020_State_x0020_Name_x005d_": "South Dakota"
        });
        assert_eq!(dim_value(&row).as_deref(), Some("South Dakota"));
    }

    #[test]
    fn dim_value_int_float_renders_as_int() {
        let row = json!({
            "_x005b_Total_x0020_Store_x0020_Sales_x005d_": 78087067.22,
            "atscale_catalogs_x005b_Sold_x0020_Calendar_x0020_Week_x005d_": 5159.0
        });
        assert_eq!(dim_value(&row).as_deref(), Some("5159"));
    }

    #[test]
    fn infer_value_type_classes() {
        assert_eq!(infer_value_type(&["Alabama".into(), "Ohio".into()]), "string");
        assert_eq!(infer_value_type(&["1".into(), "2".into(), "12".into()]), "integer");
        assert_eq!(infer_value_type(&["2001-01-15".into()]), "date");
    }
}
