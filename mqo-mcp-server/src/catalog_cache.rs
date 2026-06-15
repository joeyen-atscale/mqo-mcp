//! Catalog disk cache — persist the enriched catalog after live domain ingest
//! and validate cheaply on next startup (PRD-mqo-catalog-disk-cache).
//!
//! ## Cache file format
//!
//! A single JSON file written atomically (write-to-.tmp + rename) beside the
//! catalog snapshot (`<catalog>.enriched-cache.json`) or at a user-specified
//! path.  The file carries:
//!   * `format_version` — incremented on breaking schema changes; a mismatch
//!     triggers a full re-ingest (FR-7 / NFR-2).
//!   * `cube` — the cube name used during ingest.
//!   * `schema_update` — `LAST_SCHEMA_UPDATE` from `MDSCHEMA_CUBES` at ingest
//!     time (OQ-4: may be absent on other clusters, gracefully treated as unknown).
//!   * `captured_at` — Unix timestamp (seconds) of when the cache was written.
//!   * `columns` — the enriched `catalog["columns"]` JSON array.
//!
//! ## Validity gate (`validate_cache`)
//!
//! Three signals are checked (cheapest first):
//! 1. **TTL**: if `now - captured_at > ttl_secs` → `FullReingest`.
//! 2. **Schema timestamp**: if `LAST_SCHEMA_UPDATE` advanced → `FullReingest`.
//! 3. **Per-level cardinality diff**: levels whose `LEVEL_CARDINALITY` changed
//!    (or are new) → `PartialInvalid(changed_levels)`.
//!
//! If nothing moved → `Valid` (serve cache, skip member fetches).
//!
//! ## LAST_DATA_UPDATE
//!
//! FR-5: this column is explicitly NOT used.  It advances per-call (~wall-clock)
//! and is therefore not a discrete data-load marker.

use mqo_auth_bridge::LiveExecutor;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// Current format version. Bump when the `CatalogCache` schema changes in a
/// backwards-incompatible way; old caches will be treated as corrupt (FR-7).
pub const CACHE_FORMAT_VERSION: u32 = 1;

/// Persisted representation of the enriched catalog.
#[derive(Debug, Serialize, Deserialize)]
pub struct CatalogCache {
    /// Format guard (NFR-2).
    pub format_version: u32,
    /// Cube name recorded at ingest time.
    pub cube: String,
    /// `LAST_SCHEMA_UPDATE` from `MDSCHEMA_CUBES`.  `None` when the cluster does
    /// not populate it (OQ-4) — the gate degrades to cardinality-diff + TTL only.
    pub schema_update: Option<String>,
    /// Unix epoch seconds when this cache was written.
    pub captured_at: u64,
    /// The full `catalog["columns"]` JSON array after ingest.
    pub columns: Value,
}

/// Decision returned by `validate_cache`.
#[derive(Debug, PartialEq)]
pub enum CacheVerdict {
    /// Cache is current; serve it, skip per-level member fetches.
    Valid,
    /// One or more levels changed cardinality; re-fetch only these level
    /// `unique_name`s (the `LEVEL_UNIQUE_NAME` from `MDSCHEMA_LEVELS`).
    PartialInvalid(Vec<String>),
    /// Schema timestamp advanced, TTL expired, or cache is unusable; full
    /// re-ingest required.
    FullReingest,
}

// ── Load / save ──────────────────────────────────────────────────────────────

/// Load and deserialize the cache file.
///
/// Returns `None` on any error (missing file, truncated JSON, wrong version,
/// deserialization failure) so the caller can fall back to a full ingest
/// without crashing (FR-7).
pub fn load_cache(path: &Path) -> Option<CatalogCache> {
    let bytes = std::fs::read(path).ok()?;
    let cache: CatalogCache = serde_json::from_slice(&bytes).ok()?;
    if cache.format_version != CACHE_FORMAT_VERSION {
        eprintln!(
            "mqo-mcp-server: catalog cache: format_version {} != {}; ignoring (full re-ingest)",
            cache.format_version, CACHE_FORMAT_VERSION
        );
        return None;
    }
    Some(cache)
}

/// Serialize and write the cache atomically (write to `.tmp`, then rename).
///
/// Logs a warning on failure but never aborts — a write error must not take
/// the server down (FR-7).
pub fn save_cache(path: &Path, cache: &CatalogCache) {
    let json = match serde_json::to_vec_pretty(cache) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("mqo-mcp-server: catalog cache: serialize error: {e}");
            return;
        }
    };
    // Write to a temporary sibling, then rename atomically.
    let tmp = path.with_extension("tmp");
    if let Err(e) = std::fs::write(&tmp, &json) {
        eprintln!("mqo-mcp-server: catalog cache: write error {}: {e}", tmp.display());
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        eprintln!("mqo-mcp-server: catalog cache: rename error: {e}");
        // Best-effort cleanup; ignore secondary error.
        let _ = std::fs::remove_file(&tmp);
    }
}

// ── MDSCHEMA probes ───────────────────────────────────────────────────────────

/// Fetch `LAST_SCHEMA_UPDATE` from a single `MDSCHEMA_CUBES` Discover.
///
/// Returns `None` when the column is absent, empty, or the Discover fails.
/// The caller degrades gracefully to cardinality-diff + TTL (OQ-4).
///
/// NOTE: `LAST_DATA_UPDATE` is deliberately NOT returned here (FR-5).
pub fn fetch_schema_update(ex: &LiveExecutor, xmla_catalog: &str, cube: &str) -> Option<String> {
    let rows = ex.discover_mdschema("MDSCHEMA_CUBES", xmla_catalog, cube, None).ok()?;
    for row in &rows {
        // Locate the cube row (some clusters list multiple cubes).
        if row.get("CUBE_NAME").map(String::as_str) == Some(cube) {
            if let Some(ts) = row.get("LAST_SCHEMA_UPDATE") {
                if !ts.is_empty() {
                    return Some(ts.clone());
                }
            }
        }
    }
    // Fallback: return the first non-empty value if we didn't match by cube name.
    for row in &rows {
        if let Some(ts) = row.get("LAST_SCHEMA_UPDATE") {
            if !ts.is_empty() {
                return Some(ts.clone());
            }
        }
    }
    None
}

/// Build `{level_unique_name → cardinality}` from the two bulk Discovers
/// (`MDSCHEMA_MEASURES` is not needed here — cardinalities live in `MDSCHEMA_LEVELS`).
///
/// Returns an empty map on Discover failure (caller degrades gracefully).
pub fn ingest_cardinalities_only(
    ex: &LiveExecutor,
    xmla_catalog: &str,
    cube: &str,
) -> HashMap<String, usize> {
    let mut map = HashMap::new();
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
                let lun = r.get("LEVEL_UNIQUE_NAME").cloned().unwrap_or_default();
                if lun.is_empty() {
                    continue;
                }
                let card = r
                    .get("LEVEL_CARDINALITY")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(0);
                // Mirror the storage condition in ingest_live_metadata: only keep
                // levels with a real, non-zero, non-sentinel cardinality so the diff
                // against cardinality_map() is apples-to-apples. Over-cap / unknown
                // levels are absent from both maps and never trigger a partial re-fetch.
                if card > 0 && card < usize::MAX {
                    map.entry(lun).or_insert(card);
                }
            }
        }
        Err(e) => {
            eprintln!("mqo-mcp-server: catalog cache: MDSCHEMA_LEVELS failed: {e}");
        }
    }
    map
}

// ── Cardinality map from cached columns ──────────────────────────────────────

/// Build `{level_unique_name → cardinality}` from the `columns` array stored
/// in a `CatalogCache`.
///
/// Only `kind == "level"` columns that have a `"cardinality"` field contribute.
/// Levels without a stored cardinality are omitted (they were over-cap or
/// unmapped and will not trigger a partial re-fetch).
pub fn cardinality_map(columns: &Value) -> HashMap<String, usize> {
    let mut map = HashMap::new();
    let Some(arr) = columns.as_array() else { return map };
    for col in arr {
        if col.get("kind").and_then(Value::as_str) != Some("level") {
            continue;
        }
        let lun = col
            .get("level_unique_name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if lun.is_empty() {
            continue;
        }
        let card = col
            .get("cardinality")
            .and_then(Value::as_u64)
            .map(|v| v as usize);
        if let Some(c) = card {
            map.insert(lun.to_string(), c);
        }
    }
    map
}

// ── Validity gate ─────────────────────────────────────────────────────────────

/// Decide whether the cache is still valid.
///
/// `fresh_schema_update` — `LAST_SCHEMA_UPDATE` just fetched from the cluster
/// (may be `None` if absent or the Discover failed).
///
/// `fresh_cardinalities` — `{level_unique_name → cardinality}` from the fresh
/// `MDSCHEMA_LEVELS` bulk Discover.
///
/// `cache_cardinalities` — same map extracted from the cached columns via
/// `cardinality_map`.
pub fn validate_cache(
    cache: &CatalogCache,
    fresh_schema_update: Option<&str>,
    fresh_cardinalities: &HashMap<String, usize>,
    cache_cardinalities: &HashMap<String, usize>,
    ttl_secs: u64,
    now_secs: u64,
) -> CacheVerdict {
    // 1. TTL check (most coarse — always authoritative).
    let age = now_secs.saturating_sub(cache.captured_at);
    if age > ttl_secs {
        eprintln!(
            "mqo-mcp-server: catalog cache: TTL expired (age {}s > {}s) — full re-ingest",
            age, ttl_secs
        );
        return CacheVerdict::FullReingest;
    }

    // 2. Schema timestamp — if the cluster supplies it and it changed, the model
    //    was re-published; a full re-ingest is needed (structure may have changed).
    //    NOTE: FR-5 — LAST_DATA_UPDATE is never consulted here.
    if let (Some(fresh), Some(cached)) = (fresh_schema_update, cache.schema_update.as_deref()) {
        if fresh != cached {
            eprintln!(
                "mqo-mcp-server: catalog cache: LAST_SCHEMA_UPDATE changed \
                 ({cached} → {fresh}) — full re-ingest"
            );
            return CacheVerdict::FullReingest;
        }
    }

    // 3. Per-level cardinality diff — collect LUNs that changed or appeared.
    let mut changed: Vec<String> = Vec::new();
    for (lun, &fresh_card) in fresh_cardinalities {
        match cache_cardinalities.get(lun) {
            Some(&cached_card) if cached_card != fresh_card => {
                changed.push(lun.clone());
            }
            None => {
                // New level not in the cache → re-fetch.
                changed.push(lun.clone());
            }
            _ => {}
        }
    }

    if changed.is_empty() {
        CacheVerdict::Valid
    } else {
        CacheVerdict::PartialInvalid(changed)
    }
}

// ── Cache path helpers ────────────────────────────────────────────────────────

/// Derive the default cache path from the catalog snapshot path:
/// `<catalog>.enriched-cache.json`.
pub fn default_cache_path(catalog_path: &Path) -> std::path::PathBuf {
    let mut p = catalog_path.as_os_str().to_owned();
    p.push(".enriched-cache.json");
    std::path::PathBuf::from(p)
}

// ── Apply cached columns ──────────────────────────────────────────────────────

/// Layer the cached columns onto `catalog` in-memory.
///
/// The cached `columns` array replaces the catalog's `columns` field.  Other
/// catalog fields (cube name, hierarchy structure) remain from the snapshot.
pub fn apply_cached_columns(catalog: &mut Value, cached_columns: &Value) {
    if let Some(obj) = catalog.as_object_mut() {
        obj.insert("columns".to_string(), cached_columns.clone());
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write as _;

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn make_cache(schema_update: Option<&str>, captured_at: u64, columns: Value) -> CatalogCache {
        CatalogCache {
            format_version: CACHE_FORMAT_VERSION,
            cube: "test_cube".into(),
            schema_update: schema_update.map(|s| s.to_string()),
            captured_at,
            columns,
        }
    }

    // ── AC-6: corrupt JSON → None ─────────────────────────────────────────────
    #[test]
    fn load_cache_corrupt_json_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        std::fs::write(&path, b"not valid json {{{").unwrap();
        assert!(load_cache(&path).is_none(), "corrupt JSON must return None");
    }

    // ── AC-6: wrong format_version → None ────────────────────────────────────
    #[test]
    fn load_cache_wrong_version_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let bad = json!({
            "format_version": 9999,
            "cube": "x",
            "schema_update": null,
            "captured_at": 0,
            "columns": []
        });
        std::fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
        assert!(load_cache(&path).is_none(), "wrong format_version must return None");
    }

    // ── AC-5: TTL expired → FullReingest ──────────────────────────────────────
    #[test]
    fn validate_cache_ttl_expired() {
        let cache = make_cache(None, 1_000_000, json!([]));
        let verdict = validate_cache(
            &cache,
            None,
            &HashMap::new(),
            &HashMap::new(),
            86_400,
            1_000_000 + 86_401, // age = 86 401 > 86 400
        );
        assert_eq!(verdict, CacheVerdict::FullReingest);
    }

    // ── AC-3: one level cardinality changed → PartialInvalid ─────────────────
    #[test]
    fn validate_cache_one_level_cardinality_changed() {
        let cache = make_cache(None, 1_000_000, json!([]));
        let mut fresh = HashMap::new();
        fresh.insert("[Store].[Store].[Store State]".to_string(), 55_usize);

        let mut cached = HashMap::new();
        cached.insert("[Store].[Store].[Store State]".to_string(), 50_usize);

        let verdict = validate_cache(&cache, None, &fresh, &cached, 86_400, 1_000_100);
        match verdict {
            CacheVerdict::PartialInvalid(levels) => {
                assert_eq!(levels, vec!["[Store].[Store].[Store State]"]);
            }
            other => panic!("expected PartialInvalid, got {other:?}"),
        }
    }

    // ── AC-2: schema_update changed → FullReingest ────────────────────────────
    #[test]
    fn validate_cache_schema_update_changed() {
        let cache = make_cache(Some("2026-01-01T00:00:00Z"), 1_000_000, json!([]));
        let verdict = validate_cache(
            &cache,
            Some("2026-06-14T10:00:00Z"),
            &HashMap::new(),
            &HashMap::new(),
            86_400,
            1_000_100,
        );
        assert_eq!(verdict, CacheVerdict::FullReingest);
    }

    // ── Valid case: nothing changed ───────────────────────────────────────────
    #[test]
    fn validate_cache_valid() {
        let cache = make_cache(Some("2026-05-27T21:42:58Z"), 1_000_000, json!([]));
        let mut cardinalities = HashMap::new();
        cardinalities.insert("[Store].[Store].[Store State]".to_string(), 50_usize);

        let verdict = validate_cache(
            &cache,
            Some("2026-05-27T21:42:58Z"),
            &cardinalities,
            &cardinalities,
            86_400,
            1_000_100,
        );
        assert_eq!(verdict, CacheVerdict::Valid);
    }

    // ── AC-7: save + load round-trips correctly ───────────────────────────────
    #[test]
    fn save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");

        let columns = json!([
            {"kind": "level", "hierarchy": "store_dimension", "level": "Store State",
             "value_type": "string", "domain": ["CA", "WA"], "cardinality": 50}
        ]);
        let cache = make_cache(Some("2026-05-27T21:42:58Z"), 1_234_567, columns.clone());
        save_cache(&path, &cache);

        let loaded = load_cache(&path).expect("should load successfully");
        assert_eq!(loaded.format_version, CACHE_FORMAT_VERSION);
        assert_eq!(loaded.cube, "test_cube");
        assert_eq!(loaded.schema_update.as_deref(), Some("2026-05-27T21:42:58Z"));
        assert_eq!(loaded.captured_at, 1_234_567);
        assert_eq!(loaded.columns, columns);
    }

    // ── cardinality_map extracts from cached columns ──────────────────────────
    #[test]
    fn cardinality_map_from_columns() {
        let columns = json!([
            {"kind": "level", "level_unique_name": "[Store].[Store State]", "cardinality": 50},
            {"kind": "level", "level_unique_name": "[Ship].[Mode]"},  // no cardinality
            {"kind": "measure", "level_unique_name": "[Measures].[Sales]"},
        ]);
        let map = cardinality_map(&columns);
        assert_eq!(map.get("[Store].[Store State]"), Some(&50_usize));
        assert!(!map.contains_key("[Ship].[Mode]"), "no-cardinality level omitted");
        assert!(!map.contains_key("[Measures].[Sales]"), "measure omitted");
    }

    // ── default_cache_path ────────────────────────────────────────────────────
    #[test]
    fn default_cache_path_appends_suffix() {
        let p = std::path::Path::new("fixtures/tpcds_catalog.json");
        let cp = default_cache_path(p);
        assert_eq!(
            cp.to_str().unwrap(),
            "fixtures/tpcds_catalog.json.enriched-cache.json"
        );
    }

    // ── new level appears in fresh cardinalities → PartialInvalid ────────────
    #[test]
    fn validate_cache_new_level_triggers_partial_invalid() {
        let cache = make_cache(None, 1_000_000, json!([]));
        let mut fresh = HashMap::new();
        fresh.insert("[New].[New Level]".to_string(), 10_usize);
        // cached map is empty — level is new
        let verdict = validate_cache(&cache, None, &fresh, &HashMap::new(), 86_400, 1_000_100);
        match verdict {
            CacheVerdict::PartialInvalid(levels) => {
                assert!(levels.contains(&"[New].[New Level]".to_string()));
            }
            other => panic!("expected PartialInvalid, got {other:?}"),
        }
    }
}
