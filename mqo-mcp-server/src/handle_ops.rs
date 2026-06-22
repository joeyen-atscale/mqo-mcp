//! Handle-operation MCP tools backed by **`dh-store` + `dh-ops`** (typed
//! columnar), replacing the former `mqo-duckdb-handle-store` `MemStore`
//! Rust-over-`serde_json::Value` op path.
//!
//! Each tool operates over an in-memory result store ([`dh_store::Store`]) with
//! immutable derive-new semantics: the input handle is never mutated; a new
//! handle is minted for every derived result.  All computation is server-side
//! over typed columns — no `AtScale` engine round-trip is issued after the
//! initial `query_multidimensional` call.
//!
//! The full 10-op `dataset_*` family is exposed:
//! `aggregate, filter, sort, top_n, pivot, compare, drill, describe` (from the
//! `dh-ops` kernel) plus `slice, period_over_period` and the visualization op
//! `dataset_chart` (bespoke here).
//!
//! **Inline threshold**: each derived result's response carries a bounded
//! `summary` + the new `handle` + `row_count`; raw `rows` are inlined only when
//! `row_count <= inline_threshold` (configured per server, default 25).

// Pre-existing lint suppressions — do not remove without fixing the underlying code.
#![allow(
    clippy::absurd_extreme_comparisons,
    clippy::doc_overindented_list_items,
    clippy::explicit_auto_deref,
    clippy::items_after_statements,
    clippy::manual_let_else,
    clippy::missing_panics_doc,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::map_unwrap_or,
    clippy::uninlined_format_args,
    clippy::must_use_candidate,
    clippy::too_many_lines,
    clippy::missing_errors_doc,
    clippy::similar_names,
    clippy::redundant_closure_for_method_calls,
    clippy::doc_markdown,
    clippy::map_clone,
    clippy::used_underscore_binding,
    clippy::unnested_or_patterns,
    clippy::manual_range_patterns,
    clippy::if_not_else,
    clippy::implicit_hasher
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use dh_spec::{ColumnRole, ColumnSchema, DatasetHandle, DType};
use dh_store::{ColumnData, Dataset, Store};
use dh_summary::{capabilities as dh_capabilities, summarize, SummaryCfg};
use serde_json::{json, Map, Value};

use mqo_param_validator::{
    check_dataset_aggregate_attribute, CatalogHierarchy, CatalogMeasure, CatalogSnapshot,
};

/// Default maximum number of raw rows to inline in a handle-op or query
/// response.  Overridable at launch via `--inline-threshold`.
pub const INLINE_THRESHOLD: usize = 25;

/// TTL applied to every stored / derived dataset, in seconds.
const STORE_TTL_SECS: u64 = 3600;

/// Total byte cap for the dh-store (256 MiB).  `0` would be unlimited.
const STORE_MAX_BYTES: usize = 256 * 1024 * 1024;

// ── Store accessor type ───────────────────────────────────────────────────────

/// A shared, locked [`dh_store::Store`].
pub type SharedStore = Arc<Mutex<Store>>;

/// Public wrapper that owns the shared store and exposes the tool handlers.
pub struct HandleStore {
    /// The shared typed columnar store.
    pub store: SharedStore,
}

impl HandleStore {
    /// Create a new [`HandleStore`] backed by a freshly-allocated dh-store.
    #[must_use]
    pub fn new() -> Self {
        HandleStore {
            store: Arc::new(Mutex::new(Store::new(STORE_MAX_BYTES))),
        }
    }

    /// Ingest a set of JSON result rows (as produced by the query pipeline) into
    /// the store and return the minted handle.  Used by `query_multidimensional`
    /// to size-gate its response.
    ///
    /// # Errors
    ///
    /// Returns `Err(String)` if the store lock is poisoned.
    pub fn put_rows(&self, rows: &[Value]) -> Result<DatasetHandle, String> {
        let ds = json_rows_to_dataset(rows);
        let guard = self.store.lock().map_err(|_| "store lock poisoned".to_string())?;
        Ok(guard.put(ds, STORE_TTL_SECS))
    }

    /// Ingest result rows into the store with **bound-authoritative** column
    /// roles (see [`json_rows_to_dataset_with_bound`]) and return the minted
    /// handle.  This is the variant used by `query_multidimensional` so that
    /// the stored dataset — the one `dataset_*` ops later read — labels numeric
    /// dimensions as `Dimension`, not `Measure`.
    ///
    /// # Errors
    ///
    /// Returns `Err(String)` if the store lock is poisoned.
    pub fn put_rows_with_bound(&self, rows: &[Value], bound: &Value) -> Result<DatasetHandle, String> {
        let ds = json_rows_to_dataset_with_bound(rows, bound);
        let guard = self.store.lock().map_err(|_| "store lock poisoned".to_string())?;
        Ok(guard.put(ds, STORE_TTL_SECS))
    }

    /// Ingest result rows into the store using **canonical clean column labels**
    /// identical to those the `query_multidimensional` response uses (PRD-mqo-handle-canonical-labels,
    /// FR-1/FR-3/G1).
    ///
    /// This is the **single shared path** (FR-3): applies `clean_result_rows` (the same
    /// function that cleans the response) to normalize raw DAX-mangled column keys to
    /// canonical clean labels BEFORE persisting, so:
    ///   - handle schema == response columns (AC-1)
    ///   - `dataset_export` columns == response columns (AC-2)
    ///   - collision disambiguation is identical (FR-5, AC-3)
    ///   - already-clean names are a no-op (`clean_label(clean) == clean`, FR-7, AC-6)
    ///
    /// Column *roles* are still bound-authoritative (same as [`put_rows_with_bound`]).
    ///
    /// Back-compat (FR-6): callers that pass a legacy raw key to a `dataset_*` op
    /// will encounter an `unknown_column` error from dh-ops (the raw key is no longer
    /// stored).  The caller-side resolver in [`clean_result_rows`] remains available
    /// for ops that need to map incoming raw keys to canonical names.
    ///
    /// # Errors
    ///
    /// Returns `Err(String)` if the store lock is poisoned.
    pub fn put_rows_with_canonical_labels(
        &self,
        rows: &[Value],
        bound: &Value,
    ) -> Result<DatasetHandle, String> {
        // FR-3: reuse the SAME `clean_result_rows` function the response uses.
        // This is the single shared implementation — no fork.
        let canonical_rows = clean_result_rows(rows, bound);
        // Bound-authoritative roles over the canonical-keyed rows.
        let ds = json_rows_to_dataset_with_bound(&canonical_rows, bound);
        let guard = self.store.lock().map_err(|_| "store lock poisoned".to_string())?;
        Ok(guard.put(ds, STORE_TTL_SECS))
    }
}

impl Default for HandleStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── JSON ⇆ Dataset conversion ─────────────────────────────────────────────────

/// Infer the typed columnar [`Dataset`] from JSON object rows.
///
/// Column order and names follow the first row's key order.  Each column's
/// dtype is inferred by scanning all values (numbers → Int unless any is
/// fractional → Float; bools → Bool; otherwise Str).  Roles are heuristic:
/// numeric columns are `Measure`, everything else `Dimension`.
#[must_use]
pub fn json_rows_to_dataset(rows: &[Value]) -> Dataset {
    // Collect column names in first-seen order across rows (robust to ragged rows).
    let mut col_names: Vec<String> = Vec::new();
    for row in rows {
        if let Some(obj) = row.as_object() {
            for k in obj.keys() {
                if !col_names.iter().any(|c| c == k) {
                    col_names.push(k.clone());
                }
            }
        }
    }

    let mut columns: Vec<ColumnSchema> = Vec::with_capacity(col_names.len());
    let mut data: Vec<ColumnData> = Vec::with_capacity(col_names.len());

    for name in &col_names {
        let (dtype, role, col) = build_column(name, rows);
        columns.push(ColumnSchema {
            name: name.clone(),
            unique_name: name.clone(),
            dtype,
            nullable: true,
            role,
        });
        data.push(col);
    }

    // Empty result: produce a zero-row, zero-col dataset (still valid).
    Dataset::new(columns, data).unwrap_or_else(|_| {
        Dataset::new(Vec::new(), Vec::new()).expect("empty dataset is always valid")
    })
}

/// Like [`json_rows_to_dataset`], but sets each column's [`ColumnRole`] from the
/// MQO `bound` rather than the dtype heuristic.
///
/// This is the **bound-authoritative** variant used whenever a *query-result*
/// dataset is stored: the bound is the source of truth for whether a projected
/// column is a Measure or a Dimension, so a numeric dimension (e.g. a calendar
/// year that comes back as `Float`) is correctly labelled `Dimension` rather
/// than being mislabelled `Measure` by the "numeric → Measure" heuristic.
///
/// dtype/data inference is identical to [`json_rows_to_dataset`]; only the role
/// differs.  Columns not present in the bound fall back to the dtype heuristic.
#[must_use]
pub fn json_rows_to_dataset_with_bound(rows: &[Value], bound: &Value) -> Dataset {
    let mut col_names: Vec<String> = Vec::new();
    for row in rows {
        if let Some(obj) = row.as_object() {
            for k in obj.keys() {
                if !col_names.iter().any(|c| c == k) {
                    col_names.push(k.clone());
                }
            }
        }
    }

    // Friendly-label classifier: tolerates XMLA name-mangled row keys that do
    // NOT equal the bound's `unique_name`.  See [`bound_role_map`].
    let role_map = bound_role_map(bound, &col_names, rows);

    let mut columns: Vec<ColumnSchema> = Vec::with_capacity(col_names.len());
    let mut data: Vec<ColumnData> = Vec::with_capacity(col_names.len());

    for name in &col_names {
        let (dtype, heuristic_role, col) = build_column(name, rows);
        // Bound is authoritative; fall back to the dtype heuristic only for
        // columns the bound does not mention.
        let role = role_map.get(name).copied().unwrap_or(heuristic_role);
        columns.push(ColumnSchema {
            name: name.clone(),
            unique_name: name.clone(),
            dtype,
            nullable: true,
            role,
        });
        data.push(col);
    }

    Dataset::new(columns, data).unwrap_or_else(|_| {
        Dataset::new(Vec::new(), Vec::new()).expect("empty dataset is always valid")
    })
}

/// Undo SSAS/XMLA name-mangling: every `_xHHHH_` (4 hex digits) becomes the
/// corresponding Unicode character.  Mirrors the demo bridge's
/// `_decode_xml_name`.
fn decode_xml_name(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        // Look for the pattern `_xHHHH_` starting at byte i.
        if bytes[i] == b'_'
            && i + 6 < s.len()
            && (bytes[i + 1] == b'x' || bytes[i + 1] == b'X')
            && bytes[i + 6] == b'_'
            && bytes[i + 2..i + 6].iter().all(u8::is_ascii_hexdigit)
        {
            let hex = &s[i + 2..i + 6];
            if let Ok(code) = u32::from_str_radix(hex, 16) {
                if let Some(ch) = char::from_u32(code) {
                    out.push(ch);
                    i += 7;
                    continue;
                }
            }
        }
        // Not a mangled escape: copy this byte's char.  `s[i..]` always starts
        // on a char boundary here because escapes are pure ASCII.
        let ch = s[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Contents of the last `[...]` segment in `s`, trimmed, if any.
fn last_bracket_contents(s: &str) -> Option<String> {
    let mut last: Option<String> = None;
    let mut depth_start: Option<usize> = None;
    for (idx, ch) in s.char_indices() {
        match ch {
            '[' => depth_start = Some(idx + 1),
            ']' => {
                if let Some(start) = depth_start.take() {
                    last = Some(s[start..idx].trim().to_string());
                }
            }
            _ => {}
        }
    }
    last
}

/// Friendly label for a (possibly XML-mangled) **row column key**.  Prefers the
/// contents of the last `[...]` segment (the level/measure name); else the
/// decoded string trimmed of `._ `.  Mirrors the bridge's `_clean_label`.
///
/// Exposed as `pub(crate)` so `pipeline.rs` and the response layer can reuse
/// the same decoder (FR-2: one implementation, not two).
pub(crate) fn clean_label(raw_key: &str) -> String {
    let decoded = decode_xml_name(raw_key);
    if let Some(b) = last_bracket_contents(&decoded) {
        return b;
    }
    decoded.trim_matches(|c| c == '.' || c == '_' || c == ' ').to_string()
}

/// Collapse a **near-twin** dimension level caption to its **canonical** label.
///
/// (PRD-mqo-near-twin-dimension-drop, G2 — the canonical-output-label half.)
///
/// A "near-twin" hierarchy is a role-playing / snowflaked copy of a base
/// dimension: its level captions are the base level caption **prefixed with the
/// relationship path**. In the TPC-DS benchmark model the base
/// `product_dimension` carries `Item Product Name`, `Product Brand Name`,
/// `Product Category`, …; the twin `promotion_product_item_product_dimension`
/// carries `Promotion Product Item Item Product Name`,
/// `Promotion Product Item Product Brand Name`, … and `store_item_product_dimension`
/// carries `Store Item Product Category`, ….
///
/// When such a twin level is projected, the DAX result column is labeled with the
/// **full prefixed caption** (e.g. `Promotion Product Item Item Product Name`),
/// so an exact-name comparison against the canonical `Item Product Name` (what
/// the analyst/gold expects, and what the base hierarchy would have produced)
/// fails. This function recovers the canonical label so the projected twin
/// column matches the base attribute name.
///
/// ## Derivation (pure label logic — needs NO catalog domain metadata)
///
/// `caption` is canonical-collapsed to the **longest proper token-suffix of
/// `caption` that is itself a level caption present on some _other_ hierarchy**
/// in `all_level_captions`. "Proper" = strictly shorter than `caption`, so a
/// base-hierarchy caption (which has no shorter twin) is returned unchanged.
///
/// Examples (with the TPC-DS caption registry):
/// - `"Promotion Product Item Item Product Name"` → `"Item Product Name"`
/// - `"Store Item Product Category"` → `"Product Category"`
/// - `"Item Product Name"` (base) → `"Item Product Name"` (unchanged — no
///   shorter caption is also a level)
/// - `"Sold Calendar Year"` (unique, non-twin) → unchanged
///
/// Conservative: when no proper suffix of `caption` is a level caption elsewhere,
/// the caption is returned verbatim (unique levels are never altered, FR-4).
pub(crate) fn canonical_level_label(caption: &str, all_level_captions: &std::collections::HashSet<String>) -> String {
    let tokens: Vec<&str> = caption.split_whitespace().collect();
    if tokens.len() < 2 {
        return caption.to_string();
    }
    // Try successively shorter suffixes (i = 1 drops one leading token, …).
    // The FIRST (longest) proper suffix that is a known level caption wins.
    for i in 1..tokens.len() {
        let suffix = tokens[i..].join(" ");
        if all_level_captions.contains(&suffix) {
            return suffix;
        }
    }
    caption.to_string()
}

/// Friendly label for a bound `unique_name` (`hier.[Level]` or `model.measure`).
/// Mirrors the bridge's `_label_from_unique_name`.
pub(crate) fn label_from_unique_name(unique_name: &str) -> String {
    if let Some(b) = last_bracket_contents(unique_name) {
        return b;
    }
    let tail = unique_name.rsplit('.').next().unwrap_or(unique_name);
    tail.replace('_', " ").trim().to_string()
}

/// Remap the column keys of every row in `rows` from raw DAX-mangled keys to
/// clean semantic labels, using the MQO `bound` to prefer catalog labels when
/// available (OQ-3).
///
/// ## Strategy
///
/// 1. Collect all column keys from the first row (preserves order).
/// 2. Build a per-key label assignment using a two-pass approach:
///    a. First pass: try to match each key to a specific bound entry using
///       table-prefix alignment (the part before the first `_x005b_` or `.`
///       in the raw key identifies its dimension table / hierarchy).
///    b. If no prefix match, fall back to `clean_label(key)`.
///    c. Prefer the bound entry's `label` field when present (OQ-3 lean).
/// 3. Disambiguate collisions (FR-4): if two keys still map to the same label,
///    qualify each with the hierarchy prefix from its bound entry.
/// 4. Return new rows with keys replaced by their clean labels; values and
///    order are byte-identical (FR-3).
///
/// **`dataset_*` handle path is NOT affected** — this function is only called
/// on the direct `query_multidimensional` response, not on the stored dataset
/// that handle-ops read (FR-6).
pub(crate) fn clean_result_rows(rows: &[Value], bound: &Value) -> Vec<Value> {
    if rows.is_empty() {
        return Vec::new();
    }

    // Collect column keys from first row (insertion order).
    let col_keys: Vec<String> = rows
        .first()
        .and_then(Value::as_object)
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();

    if col_keys.is_empty() {
        return rows.to_vec();
    }

    // Flatten all bound entries into a list: (unique_name, catalog_label, hierarchy_prefix).
    // hierarchy_prefix = the part before the first `.` in unique_name (the table/dim name).
    let bound_entries: Vec<(String, String, String)> = {
        let mut entries = Vec::new();
        for section in &["dimensions", "measures"] {
            if let Some(arr) = bound.get(section).and_then(Value::as_array) {
                for entry in arr {
                    let un = entry
                        .get("unique_name")
                        .and_then(Value::as_str)
                        .or_else(|| entry.as_str());
                    if let Some(un) = un {
                        let catalog_lbl = entry
                            .get("label")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| label_from_unique_name(un));
                        let prefix = un
                            .split('.')
                            .next()
                            .unwrap_or("")
                            .to_string();
                        entries.push((un.to_string(), catalog_lbl, prefix));
                    }
                }
            }
        }
        entries
    };

    /// Extract the table-qualifier prefix from a raw DAX row key.
    ///
    /// - `product_dimension_x005b_Product_x0020_Category_x005d_` → `product_dimension`
    ///   (everything before the first `_x005b_` or `_x00`).
    /// - `_x005b_Total_x0020_Store_x0020_Sales_x005d_` → `""` (starts with escape).
    /// - `some.dotted.key` → `some` (for fixture-path rows where key = unique_name).
    fn table_prefix(key: &str) -> &str {
        // For mangled keys: prefix is everything before the first `_x` escape.
        if let Some(pos) = key.find("_x") {
            if pos > 0 {
                // Strip trailing `_` from the prefix (the separator before `_x005b_`).
                return key[..pos].trim_end_matches('_');
            }
            return "";
        }
        // For clean/fixture keys: use the part before the first `.`.
        if let Some(pos) = key.find('.') {
            return &key[..pos];
        }
        // No qualifier: the whole key (bare column name).
        key
    }

    // For each raw column key, find the best-matching bound entry and assign a label.
    // Match priority:
    //   1. Exact unique_name match (fixture path).
    //   2. Table-prefix of the raw key matches hierarchy_prefix of bound entry
    //      AND clean_label(key) == label_from_unique_name(un).
    //   3. clean_label(key) matches label_from_unique_name of any bound entry.
    //   4. clean_label(key) directly.
    let mut key_to_label: Vec<(String, String, String)> = col_keys
        .iter()
        .map(|key| {
            let key_prefix = table_prefix(key);
            let key_decoded = clean_label(key);

            // Priority 1: exact unique_name match (fixture path).
            if let Some((_, lbl, prefix)) = bound_entries.iter().find(|(un, _, _)| un == key) {
                return (key.clone(), lbl.clone(), prefix.clone());
            }

            // Priority 2: prefix + label match.
            if let Some((_, lbl, prefix)) = bound_entries.iter().find(|(un, _, p)| {
                !key_prefix.is_empty()
                    && !p.is_empty()
                    && key_prefix == p.as_str()
                    && key_decoded == label_from_unique_name(un)
            }) {
                return (key.clone(), lbl.clone(), prefix.clone());
            }

            // Priority 3: label-only match (no prefix or prefix doesn't align).
            if let Some((_, lbl, prefix)) = bound_entries
                .iter()
                .find(|(un, _, _)| key_decoded == label_from_unique_name(un))
            {
                return (key.clone(), lbl.clone(), prefix.clone());
            }

            // Priority 4: use decoded label directly (no bound match).
            // Extract the raw-key prefix as the qualifier for potential disambiguation.
            (key.clone(), key_decoded, key_prefix.to_string())
        })
        .collect();

    // Disambiguate collisions (FR-4): if two keys map to the same label,
    // qualify each with its hierarchy prefix.
    let mut seen_labels: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (_, lbl, _) in &key_to_label {
        *seen_labels.entry(lbl.clone()).or_insert(0) += 1;
    }
    let colliding: std::collections::HashSet<String> = seen_labels
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(lbl, _)| lbl)
        .collect();

    if !colliding.is_empty() {
        for (_, label, prefix) in &mut key_to_label {
            if colliding.contains(label.as_str()) {
                let human_prefix = prefix.replace('_', " ").trim().to_string();
                if !human_prefix.is_empty()
                    && human_prefix.to_lowercase() != label.to_lowercase()
                {
                    *label = format!("{human_prefix} {label}");
                }
                // If still can't qualify (no prefix or prefix == label), leave as-is.
            }
        }
    }

    // Build the final key→label map.
    let rename: std::collections::HashMap<&str, &str> = key_to_label
        .iter()
        .map(|(k, l, _)| (k.as_str(), l.as_str()))
        .collect();

    // Remap every row.
    rows.iter()
        .map(|row| {
            if let Some(obj) = row.as_object() {
                let mut new_obj = serde_json::Map::with_capacity(obj.len());
                for (k, v) in obj {
                    let new_key = rename.get(k.as_str()).copied().unwrap_or(k.as_str());
                    new_obj.insert(new_key.to_string(), v.clone());
                }
                Value::Object(new_obj)
            } else {
                row.clone()
            }
        })
        .collect()
}

/// Build a `row-column-key → ColumnRole` map from the MQO `bound`.
///
/// **Why not exact `unique_name` matching?**  For LIVE XMLA results the row
/// keys are name-mangled (e.g. `atscale_catalogs_x005b_Sold_x0020_Calendar...`)
/// and do NOT equal the bound's `unique_name`
/// (`sold_date_dimensions.[Sold Calendar Year]`), so an exact match finds
/// nothing and the numeric year falls through to the dtype heuristic → wrongly
/// `Measure`.  Instead we match on **friendly labels**, replicating the demo
/// bridge's `_normalize_response`:
///
/// 1. `clean_label` each row key (decode `_xHHHH_`, prefer last `[...]`).
/// 2. `label_from_unique_name` each bound dim/measure entry.
/// 3. Keep only bound labels that appear among the row labels (`known`); then
///    any column whose label is in neither set is a "straggler" assigned by
///    dtype (numeric → Measure, else Dimension).
/// 4. A column's role is Dimension if its `clean_label` ∈ dim_labels, Measure
///    if ∈ meas_labels.
///
/// When the bound carries no usable dim/measure labels the caller falls back to
/// the dtype heuristic for every column (this map is then empty).  The
/// simplified string-array bound shape (`{measures:["m"],dimensions:["d"]}`)
/// and the fixture shape (row keys == `unique_name`) both flow through the same
/// friendly-label path.
fn bound_role_map(
    bound: &Value,
    col_names: &[String],
    rows: &[Value],
) -> BTreeMap<String, ColumnRole> {
    // label_for[col_key] = friendly label of that row column.
    let label_for: BTreeMap<&str, String> = col_names
        .iter()
        .map(|k| (k.as_str(), clean_label(k)))
        .collect();
    let known: std::collections::BTreeSet<&str> =
        label_for.values().map(String::as_str).collect();

    // Each bound entry contributes its raw `unique_name` (for the fixture path,
    // where row keys == unique_name) and its friendly label (for the live XMLA
    // path, where row keys are name-mangled).  A bound label is "usable" only if
    // it appears among the row labels (`known`); the raw unique_name match is
    // always tried.
    let bound_entries = |key: &str| -> Vec<(String, String)> {
        bound
            .get(key)
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|entry| {
                        entry
                            .get("unique_name")
                            .and_then(Value::as_str)
                            .or_else(|| entry.as_str())
                    })
                    .map(|un| (un.to_string(), label_from_unique_name(un)))
                    .collect()
            })
            .unwrap_or_default()
    };

    let dim_entries = bound_entries("dimensions");
    let meas_entries = bound_entries("measures");

    let mut map: BTreeMap<String, ColumnRole> = BTreeMap::new();

    // No usable bound entries at all → leave the map empty so the caller uses
    // the dtype heuristic for every column.
    if dim_entries.is_empty() && meas_entries.is_empty() {
        return map;
    }

    // Usable friendly labels: only those that appear among the row labels.
    let dim_set: std::collections::BTreeSet<&str> = dim_entries
        .iter()
        .filter(|(_, lbl)| known.contains(lbl.as_str()))
        .map(|(_, lbl)| lbl.as_str())
        .collect();
    let meas_set: std::collections::BTreeSet<&str> = meas_entries
        .iter()
        .filter(|(_, lbl)| known.contains(lbl.as_str()))
        .map(|(_, lbl)| lbl.as_str())
        .collect();
    // Raw unique_name match (fixture path where row key == unique_name).
    let dim_un: std::collections::BTreeSet<&str> =
        dim_entries.iter().map(|(un, _)| un.as_str()).collect();
    let meas_un: std::collections::BTreeSet<&str> =
        meas_entries.iter().map(|(un, _)| un.as_str()).collect();

    for name in col_names {
        let lbl = label_for.get(name.as_str()).map_or("", String::as_str);
        let role = if dim_un.contains(name.as_str()) || dim_set.contains(lbl) {
            ColumnRole::Dimension
        } else if meas_un.contains(name.as_str()) || meas_set.contains(lbl) {
            ColumnRole::Measure
        } else {
            // Straggler: classify by dtype (numeric → Measure, else Dimension).
            let (_dtype, heuristic_role, _col) = build_column(name, rows);
            heuristic_role
        };
        map.insert(name.clone(), role);
    }
    map
}

// ---------------------------------------------------------------------------
// Blank-member fidelity (PRD-mqo-blank-member-answer-fidelity)
// ---------------------------------------------------------------------------

/// True when a JSON value represents a blank/unknown dimension member — a NULL
/// or an empty/whitespace string. Numbers and booleans are never blank members.
fn is_blank_member_value(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        _ => false,
    }
}

/// The row-column keys that are dimensions, per the MQO `bound` (falling back to
/// the dtype heuristic for any column the bound does not cover — same policy as
/// [`bound_role_map`]). Returns an empty vec when there are no rows/columns.
fn dimension_column_keys(rows: &[Value], bound: &Value) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    // Union of column keys across all rows (stable order: first-seen).
    let mut col_names: Vec<String> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for row in rows {
        if let Some(obj) = row.as_object() {
            for k in obj.keys() {
                if seen.insert(k.clone()) {
                    col_names.push(k.clone());
                }
            }
        }
    }
    if col_names.is_empty() {
        return Vec::new();
    }
    let roles = bound_role_map(bound, &col_names, rows);
    col_names
        .into_iter()
        .filter(|c| match roles.get(c) {
            Some(ColumnRole::Dimension) => true,
            // Measures and derived/calculated columns are not dimension members; a
            // null there is a legitimate empty cell, not an unknown-member row.
            Some(ColumnRole::Measure | ColumnRole::Derived) => false,
            // Bound carried no usable entries: dtype heuristic (non-numeric = dim).
            None => matches!(build_column(c, rows).1, ColumnRole::Dimension),
        })
        .collect()
}

/// Count result rows that carry a blank/NULL dimension member (FR1/FR3). A row
/// counts once if ANY of its dimension columns is blank. Measure-only blanks do
/// not count (a null measure is a legitimate empty cell, not an unknown member).
pub(crate) fn count_blank_dimension_member_rows(rows: &[Value], bound: &Value) -> usize {
    let dim_cols = dimension_column_keys(rows, bound);
    if dim_cols.is_empty() {
        return 0;
    }
    rows.iter()
        .filter(|row| {
            dim_cols
                .iter()
                .any(|c| is_blank_member_value(row.get(c.as_str()).unwrap_or(&Value::Null)))
        })
        .count()
}

/// Replace blank dimension cells with `caption` in-place (FR5 caption mode).
/// Only dimension columns are rewritten; measures are left untouched.
pub(crate) fn apply_blank_member_caption(rows: &mut [Value], bound: &Value, caption: &str) {
    let dim_cols = dimension_column_keys(rows, bound);
    if dim_cols.is_empty() {
        return;
    }
    for row in rows.iter_mut() {
        if let Some(obj) = row.as_object_mut() {
            for c in &dim_cols {
                let blank = obj.get(c).is_some_and(is_blank_member_value);
                if blank {
                    obj.insert(c.clone(), Value::String(caption.to_string()));
                }
            }
        }
    }
}

/// Decide a column's dtype/role and build its typed `ColumnData` from the rows.
fn build_column(name: &str, rows: &[Value]) -> (DType, ColumnRole, ColumnData) {
    let mut all_int = true;
    let mut any_num = false;
    let mut all_bool = true;
    let mut any_present = false;

    for row in rows {
        let v = row.get(name).unwrap_or(&Value::Null);
        match v {
            Value::Null => {}
            Value::Number(n) => {
                any_present = true;
                any_num = true;
                all_bool = false;
                if n.is_f64() && n.as_i64().is_none() {
                    all_int = false;
                }
            }
            Value::Bool(_) => {
                any_present = true;
                all_int = false;
            }
            _ => {
                any_present = true;
                all_int = false;
                all_bool = false;
            }
        }
    }

    if any_num {
        if all_int {
            let v: Vec<Option<i64>> = rows
                .iter()
                .map(|r| r.get(name).and_then(Value::as_i64))
                .collect();
            return (DType::Int, ColumnRole::Measure, ColumnData::Int(v));
        }
        let v: Vec<Option<f64>> = rows
            .iter()
            .map(|r| r.get(name).and_then(Value::as_f64))
            .collect();
        return (DType::Float, ColumnRole::Measure, ColumnData::Float(v));
    }

    if all_bool && any_present {
        let v: Vec<Option<bool>> = rows
            .iter()
            .map(|r| r.get(name).and_then(Value::as_bool))
            .collect();
        return (DType::Bool, ColumnRole::Dimension, ColumnData::Bool(v));
    }

    // Default: string dimension.
    let v: Vec<Option<String>> = rows
        .iter()
        .map(|r| match r.get(name) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Null) | None => None,
            Some(other) => Some(other.to_string()),
        })
        .collect();
    (DType::Str, ColumnRole::Dimension, ColumnData::Str(v))
}

/// Render a single column cell at `row_idx` back to a JSON value.
fn cell_to_json(col: &ColumnData, row_idx: usize) -> Value {
    match col {
        ColumnData::Int(v) => v
            .get(row_idx)
            .and_then(|o| *o)
            .map_or(Value::Null, Value::from),
        ColumnData::Float(v) => v
            .get(row_idx)
            .and_then(|o| *o)
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Null, Value::Number),
        ColumnData::Bool(v) => v
            .get(row_idx)
            .and_then(|o| *o)
            .map_or(Value::Null, Value::Bool),
        ColumnData::Decimal(v) | ColumnData::Str(v) | ColumnData::Date(v) | ColumnData::Time(v) => {
            v.get(row_idx)
                .and_then(|o| o.as_deref())
                .map_or(Value::Null, |s| Value::String(s.to_string()))
        }
        _ => Value::Null,
    }
}

/// Convert a [`Dataset`] back to JSON object rows (column-name keyed).
#[must_use]
pub fn dataset_to_json_rows(ds: &Dataset) -> Vec<Value> {
    let n = ds.row_count();
    (0..n)
        .map(|ri| {
            let mut obj = Map::with_capacity(ds.columns.len());
            for (ci, col) in ds.columns.iter().enumerate() {
                obj.insert(col.name.clone(), cell_to_json(&ds.data[ci], ri));
            }
            Value::Object(obj)
        })
        .collect()
}

// ── Query-path size gate (shared with mcp.rs structured_ok) ────────────────────

/// Whether a result of `row_count` rows should have its raw `rows` inlined in a
/// response, given the configured `inline_threshold` (K).
///
/// The structural anti-calculator guarantee: a result is inlined **iff**
/// `row_count <= inline_threshold`.  Above K the caller must omit `rows` and
/// rely on the handle + bounded summary instead.
#[must_use]
pub fn should_inline(row_count: usize, inline_threshold: usize) -> bool {
    row_count <= inline_threshold
}

// ── Response envelopes ─────────────────────────────────────────────────────────

/// Structured MCP error envelope (`isError: true`).
fn handle_err(code: &str, detail: &str) -> Value {
    let payload = json!({ "error": { "code": code, "detail": detail } });
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
        "structuredContent": payload,
        "isError": true
    })
}

/// Structured MCP success envelope (`isError: false`).
fn handle_ok(payload: &Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string(payload).unwrap_or_default() }],
        "structuredContent": payload,
        "isError": false
    })
}

/// Map a [`LookupError`] / [`dh_ops::OpError`] string to a structured envelope.
fn op_err_to_envelope(e: &dh_ops::OpError) -> Value {
    let code = match e {
        dh_ops::OpError::HandleNotFound(_) => "handle_not_found",
        dh_ops::OpError::BadParam(_) => "invalid_params",
        dh_ops::OpError::UnknownColumn(_) => "unknown_column",
        dh_ops::OpError::Unsupported(_) => "unsupported",
        dh_ops::OpError::Internal(_) => "internal_error",
    };
    handle_err(code, &e.to_string())
}

/// Return a typed `unknown_column` error that lists the handle's actual
/// canonical column names (FR-5).  Callers pass the name that was rejected
/// and the dataset whose columns should appear in the error.
fn unknown_column_error(rejected: &str, ds: &Dataset) -> Value {
    let actual: Vec<&str> = ds.columns.iter().map(|c| c.name.as_str()).collect();
    let payload = json!({
        "error": {
            "code": "unknown_column",
            "detail": format!(
                "column '{}' not found in handle. Use column names as returned by \
                 query_multidimensional or dataset_describe. Available columns: {:?}",
                rejected, actual
            ),
            "column": rejected,
            "available_columns": actual,
        }
    });
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
        "structuredContent": payload,
        "isError": true
    })
}

/// Try to resolve a legacy raw-key column arg to a canonical column name stored
/// in the dataset (FR-4 back-compat fallback).
///
/// Matching order:
/// 1. Exact match against stored canonical names (a no-op if already canonical).
/// 2. `clean_label(arg)` matches a stored canonical name exactly (the primary
///    legacy path: the raw XMLA key decodes to the canonical label).
/// 3. Case-insensitive match of `clean_label(arg)` against stored names.
///
/// Returns `Some((canonical_name, is_legacy))` — `is_legacy` is `true` when
/// the arg was NOT an exact match (i.e. the fallback fired and a deprecation
/// warning should be emitted).  Returns `None` when no match is found.
fn resolve_col_name<'a>(arg: &str, ds: &'a Dataset) -> Option<(&'a str, bool)> {
    // 1. Exact match: the arg is already canonical — no warning needed.
    if let Some(col) = ds.columns.iter().find(|c| c.name == arg) {
        return Some((col.name.as_str(), false));
    }
    // 2. clean_label of the arg matches a stored canonical name exactly.
    let decoded = clean_label(arg);
    if let Some(col) = ds.columns.iter().find(|c| c.name == decoded) {
        return Some((col.name.as_str(), true));
    }
    // 3. Case-insensitive fallback.
    let decoded_lower = decoded.to_lowercase();
    if let Some(col) = ds
        .columns
        .iter()
        .find(|c| c.name.to_lowercase() == decoded_lower)
    {
        return Some((col.name.as_str(), true));
    }
    None
}

/// Recursively replace every JSON string value that equals `old` with `new`.
///
/// This rewrites column-name strings embedded anywhere in the args tree
/// (e.g. inside `{"predicate": {"col": "old"}}`) without knowing the arg
/// shape of a specific op.
fn replace_string_in_value(v: Value, old: &str, new: &str) -> Value {
    match v {
        Value::String(s) if s == old => Value::String(new.to_string()),
        Value::Array(arr) => Value::Array(
            arr.into_iter()
                .map(|item| replace_string_in_value(item, old, new))
                .collect(),
        ),
        Value::Object(obj) => Value::Object(
            obj.into_iter()
                .map(|(k, val)| (k, replace_string_in_value(val, old, new)))
                .collect(),
        ),
        other => other,
    }
}

/// Build the size-gated result payload for a derived dataset.
///
/// Always carries `{new_handle, row_count, schema, summary, capabilities}`;
/// inlines `rows` only when `row_count <= inline_threshold`.
fn derived_payload(handle: &DatasetHandle, ds: &Dataset, inline_threshold: usize) -> Value {
    let row_count = ds.row_count();
    let cfg = SummaryCfg::default();
    let summary = summarize(ds, &cfg);
    let caps = dh_capabilities(ds);
    let schema: Vec<Value> = ds
        .columns
        .iter()
        .map(|c| json!({ "name": c.name, "ty": format!("{:?}", c.dtype) }))
        .collect();

    let mut payload = json!({
        "new_handle": handle.id,
        "row_count": row_count,
        "schema": schema,
        "summary": summary,
        "capabilities": caps,
    });

    if should_inline(row_count, inline_threshold) {
        payload["rows"] = Value::Array(dataset_to_json_rows(ds));
    }
    payload
}

/// Parse a handle string from `args["handle"]` and resolve it to the stored
/// dataset's [`DatasetHandle`] (reconstructed minimally for op dispatch).
fn parse_handle(args: &Value) -> Result<DatasetHandle, Value> {
    let Some(s) = args.get("handle").and_then(Value::as_str) else {
        return Err(handle_err("invalid_params", "missing required field 'handle'"));
    };
    Ok(reconstruct_handle(s))
}

/// Reconstruct a minimal [`DatasetHandle`] from an id string.  dh-store looks up
/// purely by `id`, so the other fields are not load-bearing for `get`/`derive`.
fn reconstruct_handle(id: &str) -> DatasetHandle {
    DatasetHandle {
        id: id.to_string(),
        created_at: 0,
        ttl_secs: STORE_TTL_SECS,
        derived_from: None,
    }
}

// ── dh-ops backed handlers (aggregate/filter/sort/top_n/pivot/compare/drill/describe) ──

/// Generic dispatcher for the single-handle dh-ops functions.
///
/// **Column-name contract (FR-1/FR-2):** column args MUST use the canonical
/// clean names as returned by `query_multidimensional` or `dataset_describe`.
///
/// **Back-compat fallback (FR-4):** if dh-ops returns `UnknownColumn`, this
/// dispatcher attempts to resolve the rejected name via [`resolve_col_name`]
/// (which applies `clean_label` and a case-insensitive match against the
/// handle's stored canonical names).  If a match is found the op is retried
/// once with the resolved name and a deprecation warning is included in the
/// success response.  This preserves backward compatibility with callers that
/// pass a legacy raw XMLA key, while steering them toward the canonical form.
///
/// **Unknown-column error (FR-5):** when no resolver match is found the
/// response is a typed `unknown_column` error that lists the handle's actual
/// canonical column names so the caller can self-correct in one turn.
fn run_dh_op<F>(store: &SharedStore, args: &Value, inline_threshold: usize, op: F) -> Value
where
    F: Fn(&mut Store, &DatasetHandle, &Value) -> Result<dh_spec::OpResult, dh_ops::OpError>,
{
    let handle = match parse_handle(args) {
        Ok(h) => h,
        Err(e) => return e,
    };
    let Ok(mut guard) = store.lock() else {
        return handle_err("store_error", "store lock poisoned");
    };
    match op(&mut guard, &handle, args) {
        Ok(res) => {
            // Re-fetch the derived dataset to build the size-gated payload.
            match guard.get(&res.handle) {
                Ok(ds) => handle_ok(&derived_payload(&res.handle, &ds, inline_threshold)),
                Err(e) => handle_err("internal_error", &e.to_string()),
            }
        }
        Err(dh_ops::OpError::UnknownColumn(ref bad_col)) => {
            // FR-4 / FR-5: try the back-compat resolver before giving up.
            let bad_col = bad_col.clone();
            let src_ds = match guard.get(&handle) {
                Ok(d) => d,
                Err(_) => {
                    return handle_err(
                        "handle_not_found",
                        &format!("handle '{}' not found or expired", handle.id),
                    );
                }
            };
            match resolve_col_name(&bad_col, &src_ds) {
                Some((canonical, is_legacy)) if is_legacy => {
                    // FR-4: resolved via the legacy fallback — retry with the
                    // canonical name substituted everywhere in args.
                    let fixed_args = replace_string_in_value(args.clone(), &bad_col, canonical);
                    let warning = format!(
                        "DEPRECATION: column arg '{}' was resolved to canonical name '{}' via \
                         the back-compat resolver. Use the canonical name '{}' (as returned by \
                         query_multidimensional or dataset_describe) to avoid this warning.",
                        bad_col, canonical, canonical
                    );
                    match op(&mut guard, &handle, &fixed_args) {
                        Ok(res) => {
                            match guard.get(&res.handle) {
                                Ok(ds) => {
                                    let mut payload =
                                        derived_payload(&res.handle, &ds, inline_threshold);
                                    // Surface the deprecation warning in the response.
                                    payload["warnings"] = json!([warning]);
                                    handle_ok(&payload)
                                }
                                Err(e) => handle_err("internal_error", &e.to_string()),
                            }
                        }
                        Err(e2) => {
                            // Resolver fired but retry still failed — return
                            // the enriched unknown_column error for the retry's error.
                            let retry_col = match &e2 {
                                dh_ops::OpError::UnknownColumn(c) => c.clone(),
                                _ => return op_err_to_envelope(&e2),
                            };
                            unknown_column_error(&retry_col, &src_ds)
                        }
                    }
                }
                // Exact match (already canonical, is_legacy=false) should not
                // reach here (the first op call would have succeeded).  Treat
                // as FR-5 unknown-column with column list.
                _ => unknown_column_error(&bad_col, &src_ds),
            }
        }
        Err(e) => op_err_to_envelope(&e),
    }
}

/// `dataset_aggregate` — group-by + aggregation via the dh-ops kernel.
///
/// When `catalog` is `Some`, the attribute-aggregation guard (RULE 5,
/// PRD-mqo-validator-attribute-aggregation-guard) runs before execution:
/// if the `measure` argument resolves unambiguously to a dimension level
/// (not an additive measure), the call is rejected with a `ParamRejection`
/// before any I/O. Pass `None` to skip the guard (e.g. in tests that
/// pre-date the guard or exercise the catalog-absent fail-open path).
pub fn handle_dataset_aggregate(
    store: &SharedStore,
    args: &Value,
    inline_threshold: usize,
    catalog: Option<&Value>,
) -> Value {
    let params = aggregate_args_to_params(args);

    // ── RULE 5: attribute-aggregation guard ───────────────────────────────
    if let Some(catalog_val) = catalog {
        if let Some(rejection) = attr_agg_guard(&params, catalog_val) {
            let payload = serde_json::json!({
                "error": {
                    "code": "param_rejected",
                    "detail": rejection.reason,
                    "rejection": serde_json::to_value(&rejection).unwrap_or(Value::Null)
                }
            });
            return json!({
                "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
                "structuredContent": payload,
                "isError": true
            });
        }
    }

    run_dh_op(store, &params, inline_threshold, dh_ops::aggregate)
}

/// Build a minimal `CatalogSnapshot` from the server's raw catalog JSON and
/// invoke [`check_dataset_aggregate_attribute`].  Returns `Some(rejection)`
/// when the guard fires, `None` to proceed.
///
/// The snapshot construction mirrors `pipeline.rs`'s `param_validate`:
/// measures registered under both `unique_name` and `label`; levels indexed
/// by hierarchy. Only `kind == "measure"` and `kind == "level"` columns are
/// relevant; everything else is ignored.
fn attr_agg_guard(
    params: &Value,
    catalog: &Value,
) -> Option<mqo_param_validator::ParamRejection> {
    // Extract `measure` argument (the column name to aggregate).
    let measure_col = params.get("measure").and_then(Value::as_str)?;
    // Extract `group_by` as a slice of string refs.
    let group_by_val: Vec<&str> = params
        .get("group_by")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    // Build the minimal CatalogSnapshot from the catalog JSON.
    let snapshot = build_attr_guard_snapshot(catalog);
    check_dataset_aggregate_attribute(measure_col, &group_by_val, &snapshot)
}

/// Build a minimal `CatalogSnapshot` sufficient for the attribute-aggregation
/// guard from the server's raw catalog JSON value.
fn build_attr_guard_snapshot(catalog: &Value) -> CatalogSnapshot {
    use std::collections::BTreeMap;

    let cols = match catalog.get("columns").and_then(Value::as_array) {
        Some(c) => c,
        None => return CatalogSnapshot::default(),
    };

    let mut measures: Vec<CatalogMeasure> = Vec::new();
    let mut hier_levels: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for c in cols {
        let kind = c.get("kind").and_then(Value::as_str).unwrap_or("");
        match kind {
            "measure" => {
                let un = match c.get("unique_name").and_then(Value::as_str) {
                    Some(u) if !u.is_empty() => u,
                    _ => continue,
                };
                let label = c.get("label").and_then(Value::as_str).map(str::to_string);
                measures.push(CatalogMeasure {
                    unique_name: un.to_string(),
                    label: label.clone(),
                    ..Default::default()
                });
                // Alias under label when it differs (callers often use label).
                if let Some(ref l) = label {
                    if l != un {
                        measures.push(CatalogMeasure {
                            unique_name: l.clone(),
                            label: Some(l.clone()),
                            ..Default::default()
                        });
                    }
                }
            }
            "level" => {
                let hier = c
                    .get("hierarchy")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        c.get("unique_name")
                            .and_then(Value::as_str)
                            .and_then(|un| un.split_once('.').map(|(h, _)| h.to_string()))
                    });
                let level_label = c
                    .get("level")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| c.get("label").and_then(Value::as_str).map(str::to_string));
                if let (Some(h), Some(lvl)) = (hier, level_label) {
                    let entry = hier_levels.entry(h).or_default();
                    if !entry.contains(&lvl) {
                        entry.push(lvl);
                    }
                }
            }
            _ => {}
        }
    }

    let dimensions = hier_levels
        .keys()
        .map(|h| mqo_param_validator::CatalogDimension {
            unique_name: h.clone(),
            subject_areas: Vec::new(),
        })
        .collect();
    let hierarchies: Vec<CatalogHierarchy> = hier_levels
        .into_iter()
        .map(|(h, levels)| CatalogHierarchy {
            dimension_unique_name: h.clone(),
            hierarchy_unique_name: h.clone(),
            levels,
            level_meta: vec![],
            fact_local_facts: vec![],
        })
        .collect();

    CatalogSnapshot {
        measures,
        dimensions,
        hierarchies,
        date_roles: vec![],
    }
}

/// Translate the legacy `{group_by, measures:[{col,agg}], filters}` arg shape
/// into the dh-ops `aggregate` params `{group_by, agg, measure}`.
///
/// dh-ops `aggregate` accepts a single agg+measure; when multiple measures are
/// supplied we use the first (callers wanting multiple should chain).  The
/// `handle` field is preserved.
fn aggregate_args_to_params(args: &Value) -> Value {
    // If the caller already uses dh-ops native shape (agg + measure), pass through.
    if args.get("agg").is_some() {
        return args.clone();
    }
    let mut out = Map::new();
    if let Some(h) = args.get("handle") {
        out.insert("handle".to_string(), h.clone());
    }
    if let Some(gb) = args.get("group_by") {
        out.insert("group_by".to_string(), gb.clone());
    }
    if let Some(first) = args
        .get("measures")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        if let Some(col) = first.get("col") {
            out.insert("measure".to_string(), col.clone());
        }
        let agg = first
            .get("agg")
            .and_then(Value::as_str)
            .unwrap_or("sum");
        out.insert("agg".to_string(), Value::from(agg));
    }
    Value::Object(out)
}

/// `dataset_filter` — compound AND/OR predicate filter via the dh-ops kernel.
pub fn handle_dataset_filter(store: &SharedStore, args: &Value, inline_threshold: usize) -> Value {
    run_dh_op(store, args, inline_threshold, dh_ops::filter)
}

/// `dataset_sort` — multi-key stable sort via the dh-ops kernel.
pub fn handle_dataset_sort(store: &SharedStore, args: &Value, inline_threshold: usize) -> Value {
    run_dh_op(store, args, inline_threshold, dh_ops::sort)
}

/// `dataset_top_n` — top/bottom N by measure via the dh-ops kernel.
pub fn handle_dataset_top_n(store: &SharedStore, args: &Value, inline_threshold: usize) -> Value {
    run_dh_op(store, args, inline_threshold, dh_ops::top_n)
}

/// `dataset_pivot` — crosstab via the dh-ops kernel.
pub fn handle_dataset_pivot(store: &SharedStore, args: &Value, inline_threshold: usize) -> Value {
    run_dh_op(store, args, inline_threshold, dh_ops::pivot)
}

/// `dataset_compare` — two-handle delta/pct-change via the dh-ops kernel.
pub fn handle_dataset_compare(store: &SharedStore, args: &Value, inline_threshold: usize) -> Value {
    run_dh_op(store, args, inline_threshold, dh_ops::compare)
}

/// `dataset_drill` — expand a grouped row to detail rows via lineage.
pub fn handle_dataset_drill(store: &SharedStore, args: &Value, inline_threshold: usize) -> Value {
    run_dh_op(store, args, inline_threshold, dh_ops::drill)
}

/// `dataset_describe` — per-column stats via the dh-ops kernel.
pub fn handle_dataset_describe(store: &SharedStore, args: &Value, inline_threshold: usize) -> Value {
    run_dh_op(store, args, inline_threshold, dh_ops::describe)
}

// ── dataset_slice (filter compatibility shim) ──────────────────────────────────

/// `dataset_slice` — filter rows matching all `[{col, op, value}]` predicates.
///
/// Kept for backward compatibility with the prior server API.  Translates the
/// `filters` array into a dh-ops AND predicate and delegates to `dh_ops::filter`.
pub fn handle_dataset_slice(store: &SharedStore, args: &Value, inline_threshold: usize) -> Value {
    let params = slice_args_to_filter_params(args);
    run_dh_op(store, &params, inline_threshold, dh_ops::filter)
}

/// Map the legacy slice op tokens (`=`, `!=`, `<`, …, `in`) to dh-ops ops.
fn map_slice_op(op: &str) -> &'static str {
    match op {
        "!=" | "<>" => "ne",
        "<" => "lt",
        "<=" => "le",
        ">" => "gt",
        ">=" => "ge",
        "in" => "in",
        _ => "eq",
    }
}

/// Build dh-ops `{predicate:{and:[…]}}` params from a legacy slice arg object.
fn slice_args_to_filter_params(args: &Value) -> Value {
    let filters = args
        .get("filters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let preds: Vec<Value> = filters
        .iter()
        .filter_map(|f| {
            let col = f.get("col").and_then(Value::as_str)?;
            let op = map_slice_op(f.get("op").and_then(Value::as_str).unwrap_or("="));
            let val = f.get("value").cloned().unwrap_or(Value::Null);
            Some(json!({ "col": col, "op": op, "val": val }))
        })
        .collect();
    let mut out = Map::new();
    if let Some(h) = args.get("handle") {
        out.insert("handle".to_string(), h.clone());
    }
    out.insert("predicate".to_string(), json!({ "and": preds }));
    Value::Object(out)
}

// ── dataset_period_over_period (bespoke) ───────────────────────────────────────

/// Bucket an ISO date/timestamp string by the given period specifier.
fn bucket_date(v: &str, period: &str) -> String {
    match period {
        "day" | "week" => v.get(..10).unwrap_or(v).to_string(),
        "month" => v.get(..7).unwrap_or(v).to_string(),
        "quarter" => {
            if let Some(m) = v.get(5..7).and_then(|s| s.parse::<u32>().ok()) {
                let q = (m - 1) / 3 + 1;
                format!("{}-Q{q}", v.get(..4).unwrap_or(v))
            } else {
                v.to_string()
            }
        }
        "year" => v.get(..4).unwrap_or(v).to_string(),
        _ => v.to_string(),
    }
}

/// `dataset_period_over_period` — bucket `date_col` by `period`, sum
/// `measure_cols` per bucket, add prior/delta/pct-delta columns, derive a new
/// handle.  Computed over the typed dataset, but using a JSON intermediate for
/// the LAG-style logic (kept identical to the prior behavior).
#[allow(clippy::too_many_lines)]
pub fn handle_dataset_period_over_period(
    store: &SharedStore,
    args: &Value,
    inline_threshold: usize,
) -> Value {
    let handle = match parse_handle(args) {
        Ok(h) => h,
        Err(e) => return e,
    };
    let Some(date_col) = args.get("date_col").and_then(Value::as_str) else {
        return handle_err("invalid_params", "missing required field 'date_col'");
    };
    let date_col = date_col.to_string();
    let period = args.get("period").and_then(Value::as_str).unwrap_or("year");
    let measure_cols: Vec<String> = args
        .get("measure_cols")
        .and_then(Value::as_array)
        .map_or(vec![], |a| {
            a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
        });
    if measure_cols.is_empty() {
        return handle_err("invalid_params", "measure_cols must be a non-empty array");
    }

    let Ok(guard) = store.lock() else {
        return handle_err("store_error", "store lock poisoned");
    };
    let src_ds = match guard.get(&handle) {
        Ok(d) => d,
        Err(e) => return handle_err("handle_not_found", &e.to_string()),
    };

    // FR-1/FR-5: validate that date_col and measure_cols are canonical names;
    // apply the back-compat resolver (FR-4) when a legacy raw key is passed.
    let (date_col, date_legacy_warning) = match resolve_col_name(&date_col, &src_ds) {
        None => return unknown_column_error(&date_col, &src_ds),
        Some((canonical, is_legacy)) => {
            let w = if is_legacy {
                Some(format!(
                    "DEPRECATION: date_col arg '{}' resolved to canonical name '{}' via the \
                     back-compat resolver. Use '{}' directly.",
                    date_col, canonical, canonical
                ))
            } else {
                None
            };
            (canonical.to_string(), w)
        }
    };
    let mut measure_cols_resolved: Vec<String> = Vec::with_capacity(measure_cols.len());
    let mut measure_legacy_warnings: Vec<String> = Vec::new();
    for mc in &measure_cols {
        match resolve_col_name(mc, &src_ds) {
            None => return unknown_column_error(mc, &src_ds),
            Some((canonical, is_legacy)) => {
                if is_legacy {
                    measure_legacy_warnings.push(format!(
                        "DEPRECATION: measure_cols arg '{}' resolved to canonical name '{}' via \
                         the back-compat resolver. Use '{}' directly.",
                        mc, canonical, canonical
                    ));
                }
                measure_cols_resolved.push(canonical.to_string());
            }
        }
    }
    let measure_cols = measure_cols_resolved;
    let all_warnings: Vec<String> = date_legacy_warning
        .into_iter()
        .chain(measure_legacy_warnings)
        .collect();

    let rows = dataset_to_json_rows(&src_ds);

    // Group rows by bucket (BTreeMap → naturally sorted).
    let mut bucket_rows: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for row in &rows {
        let date_str = row.get(&date_col).and_then(Value::as_str).unwrap_or("");
        let bucket = bucket_date(date_str, period);
        bucket_rows.entry(bucket).or_default().push(row);
    }
    let buckets: Vec<String> = bucket_rows.keys().cloned().collect();

    // Build output rows.
    let mut result_rows: Vec<Value> = Vec::with_capacity(buckets.len());
    for (i, bucket) in buckets.iter().enumerate() {
        let brows = &bucket_rows[bucket];
        let mut out = Map::new();
        out.insert("period_bucket".to_string(), Value::String(bucket.clone()));
        let mut current_vals: std::collections::HashMap<&str, f64> =
            std::collections::HashMap::new();
        for col in &measure_cols {
            let total: f64 = brows
                .iter()
                .filter_map(|r| r.get(col.as_str()).and_then(Value::as_f64))
                .sum();
            out.insert(col.clone(), Value::from(total));
            current_vals.insert(col.as_str(), total);
        }
        if i > 0 {
            let prior_rows = &bucket_rows[&buckets[i - 1]];
            for col in &measure_cols {
                let prior_total: f64 = prior_rows
                    .iter()
                    .filter_map(|r| r.get(col.as_str()).and_then(Value::as_f64))
                    .sum();
                let current = current_vals[col.as_str()];
                let delta = current - prior_total;
                let pct = if prior_total.abs() > f64::EPSILON {
                    Value::from((delta / prior_total) * 100.0)
                } else {
                    Value::Null
                };
                out.insert(format!("{col}_prior"), Value::from(prior_total));
                out.insert(format!("{col}_delta"), Value::from(delta));
                out.insert(format!("{col}_pct_delta"), pct);
            }
        } else {
            for col in &measure_cols {
                out.insert(format!("{col}_prior"), Value::Null);
                out.insert(format!("{col}_delta"), Value::Null);
                out.insert(format!("{col}_pct_delta"), Value::Null);
            }
        }
        result_rows.push(Value::Object(out));
    }

    let out_ds = json_rows_to_dataset(&result_rows);
    let new_handle = match guard.derive(
        &handle,
        dh_spec::Capability::Compare,
        args.clone(),
        out_ds.clone(),
        STORE_TTL_SECS,
    ) {
        Ok(h) => h,
        Err(e) => return handle_err("internal_error", &e.to_string()),
    };
    let mut payload = derived_payload(&new_handle, &out_ds, inline_threshold);
    if !all_warnings.is_empty() {
        payload["warnings"] = json!(all_warnings);
    }
    handle_ok(&payload)
}

// ── dataset_chart (bespoke; emits Vega-Lite spec, no new handle) ───────────────

/// `dataset_chart` — read at most `inline_threshold` rows from the handle and
/// emit a Vega-Lite v5 spec.  Returns the spec directly; no new handle.
pub fn handle_dataset_chart(store: &SharedStore, args: &Value, inline_threshold: usize) -> Value {
    let handle = match parse_handle(args) {
        Ok(h) => h,
        Err(e) => return e,
    };
    let chart_type = args.get("chart_type").and_then(Value::as_str).unwrap_or("bar");
    let Some(x_col) = args.get("x_col").and_then(Value::as_str) else {
        return handle_err("invalid_params", "missing required field 'x_col'");
    };
    let x_col = x_col.to_string();
    let y_cols: Vec<String> = args
        .get("y_cols")
        .and_then(Value::as_array)
        .map_or(vec![], |a| {
            a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
        });
    if y_cols.is_empty() {
        return handle_err("invalid_params", "y_cols must be a non-empty array");
    }
    let title = args.get("title").and_then(Value::as_str).unwrap_or("");
    let vl_mark = match chart_type {
        "bar" | "line" | "area" | "point" => chart_type,
        other => return handle_err("invalid_params", &format!("unsupported chart_type '{other}'")),
    };

    let ds = {
        let Ok(guard) = store.lock() else {
            return handle_err("store_error", "store lock poisoned");
        };
        match guard.get(&handle) {
            Ok(d) => d,
            Err(e) => return handle_err("handle_not_found", &e.to_string()),
        }
    };

    // FR-1/FR-5: validate x_col and y_cols against canonical handle columns;
    // apply back-compat resolver (FR-4) for legacy raw keys.
    let (x_col, x_legacy_warn) = match resolve_col_name(&x_col, &ds) {
        None => return unknown_column_error(&x_col, &ds),
        Some((canonical, is_legacy)) => {
            let w = if is_legacy {
                Some(format!(
                    "DEPRECATION: x_col arg '{}' resolved to canonical name '{}' via the \
                     back-compat resolver. Use '{}' directly.",
                    x_col, canonical, canonical
                ))
            } else {
                None
            };
            (canonical.to_string(), w)
        }
    };
    let mut y_cols_resolved: Vec<String> = Vec::with_capacity(y_cols.len());
    let mut y_legacy_warnings: Vec<String> = Vec::new();
    for yc in &y_cols {
        match resolve_col_name(yc, &ds) {
            None => return unknown_column_error(yc, &ds),
            Some((canonical, is_legacy)) => {
                if is_legacy {
                    y_legacy_warnings.push(format!(
                        "DEPRECATION: y_cols arg '{}' resolved to canonical name '{}' via the \
                         back-compat resolver. Use '{}' directly.",
                        yc, canonical, canonical
                    ));
                }
                y_cols_resolved.push(canonical.to_string());
            }
        }
    }
    let y_cols = y_cols_resolved;

    let mut rows = dataset_to_json_rows(&ds);
    rows.truncate(inline_threshold);
    let mut spec = build_vega_spec(vl_mark, &x_col, &y_cols, title, &rows);

    // Surface any deprecation warnings (FR-4).
    let all_warnings: Vec<String> = x_legacy_warn.into_iter().chain(y_legacy_warnings).collect();
    if !all_warnings.is_empty() {
        spec["warnings"] = json!(all_warnings);
    }
    handle_ok(&spec)
}

// ── dataset_export (dh-export backed; JSON/CSV/Parquet) ───────────────────────

/// Default row cap for `dataset_export` JSON mode.
///
/// Above this the tool returns a typed `result_too_large` error.  Callers may
/// pass a smaller `max_rows`; they may not exceed this cap without a server
/// rebuild.
///
/// Aligned with the materialization budget default
/// (`mqo_auth_bridge::DEFAULT_MAX_RESULT_ROWS`, PRD-mqo-handle-full-
/// materialization OQ-2): a handle can now hold up to the budget, so the JSON
/// export default must not be a *second*, lower silent clamp below the handle's
/// capacity. Operators raising `--max-result-rows` above this still get the
/// full handle via csv/parquet export (which is exempt from the JSON cap).
pub const DEFAULT_EXPORT_MAX_ROWS: usize = mqo_auth_bridge::DEFAULT_MAX_RESULT_ROWS;

/// `dataset_export` — materialize a handle out-of-band.
///
/// * `format = "json"`: returns rows as bounded JSON, capped at
///   `max_rows` (caller-supplied or [`DEFAULT_EXPORT_MAX_ROWS`]).  Above the cap
///   returns a typed `result_too_large` error.
/// * `format = "csv"` / `"parquet"`: writes a file to `destination` (or a temp
///   path), returns `{path, row_count}` — no rows inlined.
pub fn handle_dataset_export(store: &SharedStore, args: &Value) -> Value {
    let handle = match parse_handle(args) {
        Ok(h) => h,
        Err(e) => return e,
    };

    let format = args.get("format").and_then(Value::as_str).unwrap_or("json");

    // Lock the store for the duration of the export.
    let Ok(guard) = store.lock() else {
        return handle_err("store_error", "store lock poisoned");
    };

    match format {
        "json" => {
            let max_rows = args
                .get("max_rows")
                .and_then(Value::as_u64)
                .map(|n| n as usize)
                .unwrap_or(DEFAULT_EXPORT_MAX_ROWS)
                .min(DEFAULT_EXPORT_MAX_ROWS);

            // First check row count against cap without materializing rows.
            let dataset = match guard.get(&handle) {
                Ok(d) => d,
                Err(e) => return handle_err("handle_not_found", &e.to_string()),
            };
            let row_count = dataset.row_count();
            if row_count > max_rows {
                let payload = json!({
                    "error": {
                        "code": "result_too_large",
                        "detail": format!(
                            "dataset has {row_count} rows which exceeds the json export cap \
                             ({max_rows}). Use format='csv' or 'parquet' to write to a file, \
                             or apply dataset_filter/dataset_top_n to reduce the row count."
                        ),
                        "row_count": row_count,
                        "cap": max_rows,
                    }
                });
                return json!({
                    "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
                    "structuredContent": payload,
                    "isError": true
                });
            }

            use dh_export::{export, ExportDest, ExportFmt, ExportOptions};
            let fmt = ExportFmt::Json { max_rows };
            // Use a generous inline byte cap (128 MiB) — the row cap is the real guard.
            let dest = ExportDest::Inline { max_bytes: 128 * 1024 * 1024 };
            let opts = ExportOptions::default();

            match export(&*guard, &handle, fmt, dest, opts) {
                Ok(receipt) => {
                    let rows: Value = receipt
                        .inline_payload
                        .as_deref()
                        .and_then(|b| serde_json::from_slice(b).ok())
                        .unwrap_or(Value::Array(vec![]));
                    let payload = json!({
                        "handle": handle.id,
                        "format": "json",
                        "row_count": receipt.row_count,
                        "rows": rows,
                    });
                    handle_ok(&payload)
                }
                Err(dh_export::ExportError::LookupFailed(msg)) => {
                    handle_err("handle_not_found", &msg)
                }
                Err(dh_export::ExportError::JsonLimitExceeded { actual, limit }) => {
                    let payload = json!({
                        "error": {
                            "code": "result_too_large",
                            "row_count": actual,
                            "cap": limit,
                        }
                    });
                    json!({
                        "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
                        "structuredContent": payload,
                        "isError": true
                    })
                }
                Err(e) => handle_err("export_error", &e.to_string()),
            }
        }

        "csv" | "parquet" => {
            use dh_export::{export, ExportDest, ExportFmt, ExportOptions};
            use std::path::PathBuf;

            let fmt = if format == "csv" { ExportFmt::Csv } else { ExportFmt::Parquet };
            let ext = if format == "csv" { "csv" } else { "parquet" };

            let dest_path: PathBuf = args
                .get("destination")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    let tmp = std::env::temp_dir();
                    tmp.join(format!("dh-export-{}.{ext}", handle.id))
                });

            let opts = ExportOptions { overwrite: true, override_json_limit: false };

            match export(&*guard, &handle, fmt, ExportDest::File(dest_path.clone()), opts) {
                Ok(receipt) => {
                    let payload = json!({
                        "handle": handle.id,
                        "format": format,
                        "path": dest_path.to_string_lossy(),
                        "row_count": receipt.row_count,
                        "bytes": receipt.bytes,
                        "sha256": receipt.sha256,
                    });
                    handle_ok(&payload)
                }
                Err(dh_export::ExportError::LookupFailed(msg)) => {
                    handle_err("handle_not_found", &msg)
                }
                Err(dh_export::ExportError::ParquetNotEnabled) => {
                    handle_err("parquet_not_enabled",
                        "Parquet export requires the `parquet` cargo feature; \
                         use format='csv' instead or rebuild with --features parquet")
                }
                Err(e) => handle_err("export_error", &e.to_string()),
            }
        }

        other => handle_err(
            "invalid_params",
            &format!("unsupported format '{other}': must be one of json, csv, parquet"),
        ),
    }
}

/// Build a Vega-Lite v5 spec for the given chart parameters and data rows.
fn build_vega_spec(mark: &str, x_col: &str, y_cols: &[String], title: &str, rows: &[Value]) -> Value {
    let mut spec = if y_cols.len() == 1 {
        json!({
            "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
            "mark": mark,
            "data": { "values": rows },
            "encoding": {
                "x": { "field": x_col, "type": "nominal" },
                "y": { "field": y_cols[0], "type": "quantitative" }
            }
        })
    } else {
        let layer: Vec<Value> = y_cols
            .iter()
            .map(|y| {
                json!({
                    "mark": mark,
                    "encoding": {
                        "x": { "field": x_col, "type": "nominal" },
                        "y": { "field": y, "type": "quantitative" }
                    }
                })
            })
            .collect();
        json!({
            "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
            "data": { "values": rows },
            "layer": layer
        })
    };
    if !title.is_empty() {
        spec["title"] = Value::String(title.to_string());
    }
    spec
}

// ── MCP tool descriptors ───────────────────────────────────────────────────────

/// The full 10-op `dataset_*` family descriptors plus `dataset_chart`.
///
/// All carry `readOnlyHint: true`.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn handle_op_descriptors() -> Vec<Value> {
    vec![
        json!({
            "name": "dataset_aggregate",
            "description": "Aggregate a result-set handle by grouping on dimensions and rolling up a measure \
(sum/mean/min/max/count/count_distinct). Computes server-side over typed columns — no AtScale round-trip. \
Derives a new handle; returns {new_handle, row_count, summary, capabilities} (rows inlined only when \
row_count ≤ inline_threshold).\n\n\
**Column names:** use the exact column names returned by `query_multidimensional` or `dataset_describe` \
(e.g. `\"Revenue\"`, `\"Store Name\"`). Passing an unrecognised name returns a typed `unknown_column` \
error that lists the handle's actual column names.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "group_by": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Column names to group by. Use the exact names returned by query_multidimensional or dataset_describe."
                    },
                    "agg": { "type": "string", "enum": ["sum","mean","min","max","count","count_distinct"], "description": "Aggregation function." },
                    "measure": {
                        "type": "string",
                        "description": "Column to aggregate (not needed for count). Use the exact name returned by query_multidimensional or dataset_describe."
                    },
                    "measures": { "type": "array", "items": { "type": "object" }, "description": "Legacy multi-measure shape [{col,agg}]; first is used." }
                },
                "required": ["handle","group_by"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_filter",
            "description": "Filter a result-set handle by a compound AND/OR predicate over columns \
(ops: eq, ne, lt, le, gt, ge, in, contains, is_null, is_not_null). Computes server-side — no AtScale \
round-trip. Derives a new handle.\n\n\
**Column names:** the `col` field in every predicate must use the exact column name returned by \
`query_multidimensional` or `dataset_describe` (e.g. `\"Year\"`, `\"Product Category\"`). An \
unrecognised name returns a typed `unknown_column` error listing the handle's actual columns.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "predicate": {
                        "type": "object",
                        "description": "Predicate tree: {col, op, val} or {and:[…]} / {or:[…]}. \
The `col` field uses the column name exactly as returned by query_multidimensional or dataset_describe."
                    }
                },
                "required": ["handle","predicate"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_sort",
            "description": "Sort a result-set handle by one or more keys (asc/desc, stable). Computes \
server-side — no AtScale round-trip. Derives a new handle.\n\n\
**Column names:** the `col` field in each sort key must use the exact column name returned by \
`query_multidimensional` or `dataset_describe`. An unrecognised name returns a typed `unknown_column` \
error listing the handle's actual columns.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "keys": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "col": { "type": "string", "description": "Column name exactly as returned by query_multidimensional or dataset_describe." },
                                "dir": { "type": "string", "enum": ["asc","desc"] }
                            },
                            "required": ["col"]
                        },
                        "description": "Sort keys in priority order."
                    }
                },
                "required": ["handle","keys"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_top_n",
            "description": "Return the top or bottom N rows of a handle by a measure column \
(deterministic tie-break). Computes server-side — no AtScale round-trip. Derives a new handle.\n\n\
**Column names:** `measure` must use the exact column name returned by `query_multidimensional` or \
`dataset_describe`. An unrecognised name returns a typed `unknown_column` error listing the handle's \
actual columns.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "n": { "type": "integer", "description": "Number of rows to keep." },
                    "measure": {
                        "type": "string",
                        "description": "Measure column to rank by. Use the exact name returned by query_multidimensional or dataset_describe."
                    },
                    "dir": { "type": "string", "enum": ["top","bottom"], "description": "Top (default) or bottom." }
                },
                "required": ["handle","n","measure"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_pivot",
            "description": "Pivot a handle's rows × columns × measure into a crosstab. Computes \
server-side — no AtScale round-trip. Derives a new handle.\n\n\
**Column names:** `row_dim`, `col_dim`, and `measure` must use the exact column names returned by \
`query_multidimensional` or `dataset_describe`. An unrecognised name returns a typed `unknown_column` \
error listing the handle's actual columns.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "row_dim": {
                        "type": "string",
                        "description": "Column for pivot rows. Use the exact name returned by query_multidimensional or dataset_describe."
                    },
                    "col_dim": {
                        "type": "string",
                        "description": "Column for pivot columns. Use the exact name returned by query_multidimensional or dataset_describe."
                    },
                    "measure": {
                        "type": "string",
                        "description": "Measure to aggregate per cell. Use the exact name returned by query_multidimensional or dataset_describe."
                    },
                    "agg": { "type": "string", "enum": ["sum","mean","min","max","count"], "description": "Cell aggregation (default sum)." }
                },
                "required": ["handle","row_dim","col_dim","measure"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_compare",
            "description": "Compare two handles by joining on keys and computing delta + pct-change for \
a measure. Computes server-side — no AtScale round-trip. Derives a new handle (multi-parent lineage).\n\n\
**Column names:** `join_keys` elements and `measure` must use the exact column names returned by \
`query_multidimensional` or `dataset_describe`. An unrecognised name returns a typed `unknown_column` \
error listing the handle's actual columns.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the first (A) result." },
                    "handle_b": { "type": "object", "description": "The second result's full DatasetHandle JSON." },
                    "join_keys": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Columns to join on. Use exact names returned by query_multidimensional or dataset_describe."
                    },
                    "measure": {
                        "type": "string",
                        "description": "Measure column to diff. Use the exact name returned by query_multidimensional or dataset_describe."
                    }
                },
                "required": ["handle","handle_b","join_keys","measure"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_drill",
            "description": "Expand a grouped row of a handle back to its constituent detail rows via \
lineage. Computes server-side — no AtScale round-trip. Derives a new handle.\n\n\
**Column names:** keys in `group_row` must use the exact column names returned by \
`query_multidimensional` or `dataset_describe`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the grouped result." },
                    "group_row": {
                        "type": "object",
                        "description": "Column→value map identifying the group to drill into. \
Keys must be the exact column names returned by query_multidimensional or dataset_describe."
                    }
                },
                "required": ["handle","group_row"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_describe",
            "description": "Produce per-column stats (min/max/sum/mean/distinct) for a handle without \
changing rows. Computes server-side — no AtScale round-trip. Derives a new handle. The resulting \
schema lists the canonical column names to use in all subsequent `dataset_*` ops.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "topk": { "type": "integer", "description": "Top-k cardinality to consider (default 10)." }
                },
                "required": ["handle"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_slice",
            "description": "Filter a result-set handle to rows matching all supplied [{col, op, value}] \
filters (op: =, !=, <, <=, >, >=, in). Compatibility alias for dataset_filter. row_count=0 for no \
matches is not an error. Computes server-side — no AtScale round-trip. Derives a new handle.\n\n\
**Column names:** the `col` field in each filter must use the exact column name returned by \
`query_multidimensional` or `dataset_describe`. An unrecognised name returns a typed `unknown_column` \
error listing the handle's actual columns.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "filters": {
                        "type": "array",
                        "items": { "type": "object" },
                        "description": "Row filters: [{col, op, value}]. `col` uses the exact name returned by query_multidimensional or dataset_describe."
                    }
                },
                "required": ["handle","filters"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_period_over_period",
            "description": "Compute period-over-period deltas by bucketing date_col by period, summing \
measure_cols per bucket, and adding prior-period value, absolute delta, and percentage delta columns. \
Computes server-side — no AtScale round-trip. Derives a new handle.\n\n\
**Column names:** `date_col` and every element of `measure_cols` must use the exact column names \
returned by `query_multidimensional` or `dataset_describe`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "date_col": {
                        "type": "string",
                        "description": "Column containing date/timestamp values. Use the exact name returned by query_multidimensional or dataset_describe."
                    },
                    "period": { "type": "string", "enum": ["day","week","month","quarter","year"], "description": "Bucketing period." },
                    "measure_cols": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Measure columns to aggregate and compare across periods. Use exact names returned by query_multidimensional or dataset_describe."
                    }
                },
                "required": ["handle","date_col","period","measure_cols"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_chart",
            "description": "Produce a Vega-Lite v5 JSON spec from a result handle. Reads at most \
inline_threshold rows for inline data.values. Returns the spec directly — no new handle. Computes \
server-side — no AtScale round-trip.\n\n\
**Column names:** `x_col` and every element of `y_cols` must use the exact column names returned by \
`query_multidimensional` or `dataset_describe`. An unrecognised name returns a typed `unknown_column` \
error listing the handle's actual columns.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "chart_type": { "type": "string", "enum": ["bar","line","area","point"], "description": "Vega-Lite mark type." },
                    "x_col": {
                        "type": "string",
                        "description": "Column to bind to the x-axis. Use the exact name returned by query_multidimensional or dataset_describe."
                    },
                    "y_cols": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Column(s) to bind to the y-axis. Use exact names returned by query_multidimensional or dataset_describe."
                    },
                    "title": { "type": "string", "description": "Optional chart title." }
                },
                "required": ["handle","chart_type","x_col","y_cols"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_export",
            "description": "Materialize a result handle out-of-band. This is the deliberate, audited exit for full row data — use it when the user genuinely needs all rows (e.g. CSV download, eval harness ground-truth). \
\n\n**Large results:** when query_multidimensional returns a handle with row_count above the inline threshold, work with the handle via dataset_* ops first. Only call dataset_export when you need the full materialized rows. \
\n\n**Formats:** json (returns rows inline, bounded by max_rows cap — use for programmatic access), csv (writes a file, no rows inlined), parquet (writes a file, no rows inlined). \
\n\n**JSON cap:** json mode is bounded by the operator row cap; if the handle's row_count exceeds the cap a result_too_large error is returned instead of rows. csv/parquet write to a file and are exempt from the json cap. \
\n\nReturns {path, row_count} for csv/parquet; {rows, row_count} for json within cap. Expired/unknown handles return a typed handle_not_found error. Read-only by construction — computes server-side, no AtScale round-trip.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the result to export (from query_multidimensional or a dataset_* op)." },
                    "format": {
                        "type": "string",
                        "enum": ["json", "csv", "parquet"],
                        "description": "Export format. json: returns bounded rows inline (capped at max_rows). csv/parquet: writes a file and returns {path, row_count}."
                    },
                    "max_rows": {
                        "type": "integer",
                        "description": "For json format: maximum rows to return. Defaults to the operator row cap. Above the cap returns result_too_large.",
                        "minimum": 1
                    },
                    "destination": {
                        "type": "string",
                        "description": "For csv/parquet: file path to write to (absolute). Defaults to a temp path if omitted."
                    }
                },
                "required": ["handle", "format"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Near-twin captions (the prefixed role-playing copies) collapse to the
    /// canonical base attribute; base/unique levels are returned unchanged.
    /// (PRD-mqo-near-twin-dimension-drop, G2 — canonical output labels.)
    #[test]
    fn canonical_level_label_collapses_near_twins() {
        // The model's full level-caption registry (base + two twins).
        let captions: std::collections::HashSet<String> = [
            // Base product_dimension levels.
            "Item Product Name",
            "Product Brand Name",
            "Product Category",
            // promotion_product_item_product_dimension twin captions.
            "Promotion Product Item Item Product Name",
            "Promotion Product Item Product Brand Name",
            "Promotion Product Item Product Category",
            // store_item_product_dimension twin captions.
            "Store Item Item Product Name",
            "Store Item Product Brand Name",
            "Store Item Product Category",
            // A genuinely unique, non-twin level.
            "Sold Calendar Year",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect();

        // Failure-1 twin (project): the prefixed twin name collapses to the base.
        assert_eq!(
            canonical_level_label("Promotion Product Item Item Product Name", &captions),
            "Item Product Name"
        );
        // Failure-1 filter level twin.
        assert_eq!(
            canonical_level_label("Promotion Product Item Product Brand Name", &captions),
            "Product Brand Name"
        );
        // Failure-2 twin: store-prefixed category collapses to the base category.
        assert_eq!(
            canonical_level_label("Store Item Product Category", &captions),
            "Product Category"
        );

        // Base levels (no shorter twin) are unchanged — FR-4 (no regression).
        assert_eq!(canonical_level_label("Item Product Name", &captions), "Item Product Name");
        assert_eq!(canonical_level_label("Product Category", &captions), "Product Category");
        // Unique non-twin level is unchanged.
        assert_eq!(canonical_level_label("Sold Calendar Year", &captions), "Sold Calendar Year");
        // Single-token caption never collapses.
        assert_eq!(canonical_level_label("Category", &captions), "Category");
    }

    /// End-to-end label path: a near-twin level projected via `query_multidimensional`
    /// surfaces its CANONICAL column name (`Item Product Name`), not the prefixed
    /// twin caption — because the bound entry carries the canonical `label` that
    /// `clean_result_rows` prefers. This is the result the gold exact-name
    /// comparison needs (corpcorp-brand-products / product-count-per-category).
    #[test]
    fn clean_result_rows_emits_canonical_near_twin_label() {
        // Bound shape as produced by the binder + `attach_canonical_dimension_labels`:
        // the dimension's unique_name is the prefixed twin caption, but the entry
        // now carries the canonical `label`.
        let twin_un = "promotion_product_item_product_dimension.[Promotion Product Item Item Product Name]";
        let bound = json!({
            "measures": [{ "unique_name": "tpcds_benchmark_model.total_product_count", "label": "Total Product Count" }],
            "dimensions": [{ "unique_name": twin_un, "label": "Item Product Name" }],
        });
        // Live DAX result keys are XMLA-name-mangled from the twin caption.
        let dim_key = "promotion_product_item_product_dimension_x005b_Promotion_x0020_Product_x0020_Item_x0020_Item_x0020_Product_x0020_Name_x005d_";
        let meas_key = "_x005b_Total_x0020_Product_x0020_Count_x005d_";
        let rows = vec![
            json!({ dim_key: "AAA", meas_key: 3.0 }),
            json!({ dim_key: "BBB", meas_key: 5.0 }),
        ];

        let cleaned = clean_result_rows(&rows, &bound);
        let first = cleaned[0].as_object().expect("row is object");
        // The dimension column is named by its CANONICAL label, not the twin caption.
        assert!(
            first.contains_key("Item Product Name"),
            "expected canonical 'Item Product Name' column, got keys: {:?}",
            first.keys().collect::<Vec<_>>()
        );
        assert!(
            !first.contains_key("Promotion Product Item Item Product Name"),
            "the prefixed twin caption must NOT appear as a column name"
        );
        assert_eq!(first["Item Product Name"], json!("AAA"));
        assert!(first.contains_key("Total Product Count"));
    }

    /// AC: column roles come from the `bound`, not the value dtype.  A numeric
    /// (Float/Int) column the bound marks as a **dimension** must be labelled
    /// `Dimension`, while a numeric measure stays `Measure`.  This is the
    /// regression for "Total Store Sales by year", where the year (a Float
    /// dimension) was mislabelled `Measure` by the dtype heuristic.
    #[test]
    fn numeric_dimension_role_from_bound() {
        // Real binder bound shape: arrays of objects keyed by `unique_name`,
        // and the row keys equal those `unique_name`s.
        let year_key = "atscale.Sold_Calendar_Year";
        let sales_key = "store_sales.total_store_sales";
        let bound = json!({
            "measures": [{ "unique_name": sales_key }],
            "dimensions": [{ "unique_name": year_key }],
        });
        let rows = vec![
            json!({ year_key: 2021.0, sales_key: 100.5 }),
            json!({ year_key: 2022.0, sales_key: 200.5 }),
        ];

        let ds = json_rows_to_dataset_with_bound(&rows, &bound);

        let year = ds
            .columns
            .iter()
            .find(|c| c.name == year_key)
            .expect("year column present");
        let sales = ds
            .columns
            .iter()
            .find(|c| c.name == sales_key)
            .expect("sales column present");

        // Numeric dimension → Dimension (NOT Measure), despite Float dtype.
        assert_eq!(year.dtype, DType::Float);
        assert_eq!(year.role, ColumnRole::Dimension);
        // Numeric measure stays Measure.
        assert_eq!(sales.dtype, DType::Float);
        assert_eq!(sales.role, ColumnRole::Measure);

        // Sanity: the plain heuristic mislabels the dimension as Measure.
        let heuristic = json_rows_to_dataset(&rows);
        let h_year = heuristic
            .columns
            .iter()
            .find(|c| c.name == year_key)
            .expect("year column present");
        assert_eq!(h_year.role, ColumnRole::Measure);
    }

    /// An Int dimension is also bound-authoritative, and a column absent from
    /// the bound falls back to the dtype heuristic.
    #[test]
    fn int_dimension_from_bound_and_unknown_falls_back() {
        let bound = json!({
            "measures": [{ "unique_name": "m" }],
            "dimensions": [{ "unique_name": "yr" }],
        });
        let rows = vec![
            json!({ "yr": 2021, "m": 10, "extra_num": 7 }),
            json!({ "yr": 2022, "m": 20, "extra_num": 8 }),
        ];
        let ds = json_rows_to_dataset_with_bound(&rows, &bound);
        let cols = ds.columns;
        let role = |n: &str| cols.iter().find(|c| c.name == n).unwrap().role;
        assert_eq!(role("yr"), ColumnRole::Dimension); // Int dimension
        assert_eq!(role("m"), ColumnRole::Measure);
        // Not in bound → dtype heuristic → numeric → Measure.
        assert_eq!(role("extra_num"), ColumnRole::Measure);
    }

    /// LIVE XMLA regression: row keys are SSAS name-mangled (`_xHHHH_`) and do
    /// NOT equal the bound's `unique_name`.  Both columns arrive as `Float`, so
    /// the dtype heuristic alone would mislabel the numeric **year** dimension
    /// as a Measure.  The friendly-label classifier must recover:
    ///   - `[Sold Calendar Year]` (bracket, case-preserved) matches the bound
    ///     dimension's label → Dimension.
    ///   - the measure's dotted `unique_name` lowercases to "total store sales"
    ///     ≠ the row label "Total Store Sales", so it is dropped, then re-added
    ///     as a numeric straggler → Measure.
    ///
    /// Real shapes captured from a live mcp-aws run.
    #[test]
    fn live_xmla_mangled_keys_role_from_bound() {
        let sales_key = "_x005b_Total_x0020_Store_x0020_Sales_x005d_";
        let year_key = "atscale_catalogs_x005b_Sold_x0020_Calendar_x0020_Year_x005d_";
        let bound = json!({
            "measures": [{ "unique_name": "tpcds_benchmark_model.total_store_sales" }],
            "dimensions": [{
                "unique_name": "sold_date_dimensions.[Sold Calendar Year]",
                "hierarchy": "sold_date_dimensions"
            }],
        });
        // Both values are numeric (Float) — only the role should differ.
        let rows = vec![
            json!({ year_key: 2021.0, sales_key: 100.5 }),
            json!({ year_key: 2022.0, sales_key: 200.5 }),
        ];

        let ds = json_rows_to_dataset_with_bound(&rows, &bound);
        let col = |n: &str| ds.columns.iter().find(|c| c.name == n).unwrap();

        // Columns are NOT renamed — the raw mangled key is preserved.
        assert_eq!(col(year_key).dtype, DType::Float);
        assert_eq!(col(sales_key).dtype, DType::Float);

        // The mangled measure key → Measure (numeric straggler re-add).
        assert_eq!(col(sales_key).role, ColumnRole::Measure);
        // The mangled year key → Dimension (bracket-label match), despite Float.
        assert_eq!(col(year_key).role, ColumnRole::Dimension);
    }

    /// The simplified string-array bound shape (used by some tests) still maps.
    #[test]
    fn string_array_bound_shape_maps_roles() {
        let bound = json!({ "measures": ["revenue"], "dimensions": ["year"] });
        let rows = vec![json!({ "year": 2021.0, "revenue": 100.0 })];
        let ds = json_rows_to_dataset_with_bound(&rows, &bound);
        let cols = ds.columns;
        let role = |n: &str| cols.iter().find(|c| c.name == n).unwrap().role;
        assert_eq!(role("year"), ColumnRole::Dimension);
        assert_eq!(role("revenue"), ColumnRole::Measure);
    }

    // ── clean_result_rows tests (PRD-mqo-clean-result-labels ACs) ────────────

    /// AC-1: DAX-mangled column keys are cleaned to human-readable semantic labels.
    /// Real keys from the `product-count-per-category` failure case.
    #[test]
    fn ac1_mangled_keys_become_semantic_labels() {
        let cat_key = "product_dimension_x005b_Product_x0020_Category_x005d_";
        let cnt_key = "_x005b_Total_x0020_Product_x0020_Count_x005d_";
        let rows = vec![
            json!({ cat_key: "Books", cnt_key: 42 }),
            json!({ cat_key: "Electronics", cnt_key: 17 }),
        ];
        let bound = json!({
            "dimensions": [{ "unique_name": "product_dimension.[Product Category]" }],
            "measures": [{ "unique_name": "catalog.total_product_count" }],
        });
        let cleaned = clean_result_rows(&rows, &bound);
        assert_eq!(cleaned.len(), 2);
        let first = cleaned[0].as_object().unwrap();
        assert!(
            first.contains_key("Product Category"),
            "Expected 'Product Category', got: {:?}",
            first.keys().collect::<Vec<_>>()
        );
        assert!(
            first.contains_key("Total Product Count"),
            "Expected 'Total Product Count', got: {:?}",
            first.keys().collect::<Vec<_>>()
        );
    }

    /// AC-2: Values and column order are unchanged; only names change.
    #[test]
    fn ac2_values_and_order_unchanged() {
        let cat_key = "product_dimension_x005b_Product_x0020_Category_x005d_";
        let cnt_key = "_x005b_Total_x0020_Product_x0020_Count_x005d_";
        let rows = vec![
            json!({ cat_key: "Books", cnt_key: 42 }),
            json!({ cat_key: "Electronics", cnt_key: 17 }),
        ];
        let bound = json!({
            "dimensions": [{ "unique_name": "product_dimension.[Product Category]" }],
            "measures": [{ "unique_name": "catalog.total_product_count" }],
        });
        let cleaned = clean_result_rows(&rows, &bound);
        assert_eq!(cleaned.len(), 2);
        let r0 = cleaned[0].as_object().unwrap();
        let r1 = cleaned[1].as_object().unwrap();
        // Values are preserved.
        assert_eq!(r0.get("Product Category").and_then(|v| v.as_str()), Some("Books"));
        assert_eq!(r0.get("Total Product Count").and_then(|v| v.as_i64()), Some(42));
        assert_eq!(r1.get("Product Category").and_then(|v| v.as_str()), Some("Electronics"));
        assert_eq!(r1.get("Total Product Count").and_then(|v| v.as_i64()), Some(17));
        // Column order: Product Category before Total Product Count.
        let keys0: Vec<&str> = r0.keys().map(String::as_str).collect();
        assert_eq!(keys0.first().copied(), Some("Product Category"));
        assert_eq!(keys0.get(1).copied(), Some("Total Product Count"));
    }

    /// AC-3: Two columns that clean to the same label get distinct names.
    #[test]
    fn ac3_collision_gets_distinct_names() {
        // Both columns decode to "City" without qualification.
        let cust_city_key = "customer_dimension_x005b_City_x005d_";
        let store_city_key = "store_dimension_x005b_City_x005d_";
        let rows = vec![json!({ cust_city_key: "New York", store_city_key: "Boston" })];
        let bound = json!({
            "dimensions": [
                { "unique_name": "customer_dimension.[City]" },
                { "unique_name": "store_dimension.[City]" }
            ],
            "measures": [],
        });
        let cleaned = clean_result_rows(&rows, &bound);
        assert_eq!(cleaned.len(), 1);
        let row = cleaned[0].as_object().unwrap();
        // Must have 2 distinct keys (no collision).
        assert_eq!(row.len(), 2, "Expected 2 distinct columns, got: {:?}", row.keys().collect::<Vec<_>>());
        let keys: Vec<&str> = row.keys().map(String::as_str).collect();
        assert!(
            keys[0] != keys[1],
            "Collision: both columns got the same name '{}'", keys[0]
        );
    }

    /// AC-4: Already-clean column names pass through unchanged.
    #[test]
    fn ac4_already_clean_passthrough() {
        let rows = vec![
            json!({ "Product Category": "Books", "Revenue": 100.0 }),
        ];
        let bound = json!({
            "dimensions": [{ "unique_name": "product_dimension.[Product Category]" }],
            "measures": [{ "unique_name": "sales.Revenue" }],
        });
        let cleaned = clean_result_rows(&rows, &bound);
        let row = cleaned[0].as_object().unwrap();
        assert!(
            row.contains_key("Product Category"),
            "Expected 'Product Category' to pass through, got: {:?}", row.keys().collect::<Vec<_>>()
        );
        assert!(
            row.contains_key("Revenue"),
            "Expected 'Revenue' to pass through, got: {:?}", row.keys().collect::<Vec<_>>()
        );
    }

    /// AC-5 (PRD-mqo-clean-result-labels): `json_rows_to_dataset_with_bound`
    /// itself does NOT rename columns — it is the lower-level function used by
    /// the persist path after names are already canonical.  Calling it directly
    /// with mangled keys yields raw column names in the dataset; that is expected
    /// and tested here so we have a stable baseline for the lower-level function.
    ///
    /// NOTE: `put_rows_with_canonical_labels` (used by the live query path) applies
    /// `clean_result_rows` BEFORE calling this function, so the handle store now
    /// receives canonical names (see canonical-labels AC tests below).
    #[test]
    fn ac5_json_rows_to_dataset_with_bound_preserves_raw_keys() {
        let cat_key = "product_dimension_x005b_Product_x0020_Category_x005d_";
        let rows = vec![json!({ cat_key: "Books" })];
        let bound = json!({
            "dimensions": [{ "unique_name": "product_dimension.[Product Category]" }],
            "measures": [],
        });
        // `json_rows_to_dataset_with_bound` alone does NOT clean column names —
        // only the role is bound-authoritative.
        let ds = json_rows_to_dataset_with_bound(&rows, &bound);
        let has_raw_key = ds.columns.iter().any(|c| c.name == cat_key);
        assert!(
            has_raw_key,
            "json_rows_to_dataset_with_bound should preserve raw key '{}', got: {:?}",
            cat_key,
            ds.columns.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
    }

    // ── Canonical-labels AC tests (PRD-mqo-handle-canonical-labels v0.32.0) ──

    /// AC-1: The handle stores the same canonical clean labels the response uses.
    ///
    /// Given a query whose response columns are `["Year", "Revenue"]`, when the
    /// result is persisted via `put_rows_with_canonical_labels`, the handle's
    /// dataset column names must be exactly `["Year", "Revenue"]`.
    #[test]
    fn canonical_ac1_handle_stores_canonical_labels() {
        use crate::handle_ops::HandleStore;

        let year_key = "atscale_catalogs_x005b_Sold_x0020_Calendar_x0020_Year_x005d_";
        let revenue_key = "_x005b_Revenue_x005d_";
        let bound = json!({
            "dimensions": [{ "unique_name": "sold_date.[Sold Calendar Year]", "label": "Year" }],
            "measures": [{ "unique_name": "sales.Revenue", "label": "Revenue" }],
        });
        let rows = vec![
            json!({ year_key: 2021.0, revenue_key: 100.5 }),
            json!({ year_key: 2022.0, revenue_key: 200.5 }),
        ];

        let hs = HandleStore::new();
        let handle = hs
            .put_rows_with_canonical_labels(&rows, &bound)
            .expect("put succeeds");

        let guard = hs.store.lock().unwrap();
        let ds = guard.get(&handle).expect("handle present");

        let col_names: Vec<&str> = ds.columns.iter().map(|c| c.name.as_str()).collect();
        assert!(
            col_names.contains(&"Year"),
            "Expected 'Year' in column names, got: {col_names:?}"
        );
        assert!(
            col_names.contains(&"Revenue"),
            "Expected 'Revenue' in column names, got: {col_names:?}"
        );
        assert!(
            !col_names.iter().any(|n| n.contains("_x005b_") || n.contains("_x0020_")),
            "Handle must not store mangled keys, got: {col_names:?}"
        );
    }

    /// AC-2: Handle columns == response columns → `dataset_export` columns == response.
    ///
    /// The handle's column names equal what `clean_result_rows` produces for the
    /// same rows+bound (the response path), so `dataset_export` output == response.
    #[test]
    fn canonical_ac2_handle_columns_match_response_columns() {
        let cat_key = "product_dimension_x005b_Product_x0020_Category_x005d_";
        let cnt_key = "_x005b_Total_x0020_Product_x0020_Count_x005d_";
        let bound = json!({
            "dimensions": [{ "unique_name": "product_dimension.[Product Category]" }],
            "measures": [{ "unique_name": "catalog.total_product_count" }],
        });
        let rows = vec![
            json!({ cat_key: "Books", cnt_key: 42 }),
            json!({ cat_key: "Electronics", cnt_key: 17 }),
        ];

        // Response path:
        let response_cols: Vec<String> = clean_result_rows(&rows, &bound)
            .first()
            .and_then(Value::as_object)
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default();

        // Handle persist path:
        use crate::handle_ops::HandleStore;
        let hs = HandleStore::new();
        let handle = hs
            .put_rows_with_canonical_labels(&rows, &bound)
            .expect("put succeeds");
        let guard = hs.store.lock().unwrap();
        let ds = guard.get(&handle).expect("handle present");
        let handle_cols: Vec<&str> = ds.columns.iter().map(|c| c.name.as_str()).collect();

        for col in &response_cols {
            assert!(
                handle_cols.contains(&col.as_str()),
                "Handle missing response column '{col}'; handle has: {handle_cols:?}"
            );
        }
    }

    /// AC-3: Collision disambiguation in the handle matches the response exactly.
    ///
    /// Two columns that both clean to "City" get distinct qualified names; the
    /// handle's names must equal the response's disambiguated names.
    #[test]
    fn canonical_ac3_collision_disambiguation_matches_response() {
        let cust_city_key = "customer_dimension_x005b_City_x005d_";
        let store_city_key = "store_dimension_x005b_City_x005d_";
        let bound = json!({
            "dimensions": [
                { "unique_name": "customer_dimension.[City]" },
                { "unique_name": "store_dimension.[City]" }
            ],
            "measures": [],
        });
        let rows = vec![json!({ cust_city_key: "New York", store_city_key: "Boston" })];

        // Response path produces disambiguated names:
        let response_cols: Vec<String> = clean_result_rows(&rows, &bound)
            .first()
            .and_then(Value::as_object)
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default();

        // Handle persist path must match:
        use crate::handle_ops::HandleStore;
        let hs = HandleStore::new();
        let handle = hs
            .put_rows_with_canonical_labels(&rows, &bound)
            .expect("put succeeds");
        let guard = hs.store.lock().unwrap();
        let ds = guard.get(&handle).expect("handle present");
        let handle_cols: Vec<&str> = ds.columns.iter().map(|c| c.name.as_str()).collect();

        // Both paths must produce 2 distinct keys.
        assert_eq!(response_cols.len(), 2, "Response must have 2 distinct keys: {response_cols:?}");
        assert_eq!(handle_cols.len(), 2, "Handle must have 2 distinct keys: {handle_cols:?}");

        // They must be identical.
        for col in &response_cols {
            assert!(
                handle_cols.contains(&col.as_str()),
                "Handle missing disambiguated column '{col}'; handle: {handle_cols:?}"
            );
        }
    }

    /// AC-4 (exposed vs internal): The exposed column name is the canonical label
    /// even if it contains spaces (DuckDB-legal quoting is internal only).
    ///
    /// dh-store stores the column under the canonical name as-is; the exposed
    /// `name` field in the ColumnSchema is the canonical label (e.g. "Product Category").
    #[test]
    fn canonical_ac4_canonical_name_with_spaces_is_exposed() {
        let cat_key = "product_dimension_x005b_Product_x0020_Category_x005d_";
        let bound = json!({
            "dimensions": [{ "unique_name": "product_dimension.[Product Category]" }],
            "measures": [],
        });
        let rows = vec![json!({ cat_key: "Books" })];

        use crate::handle_ops::HandleStore;
        let hs = HandleStore::new();
        let handle = hs
            .put_rows_with_canonical_labels(&rows, &bound)
            .expect("put succeeds");
        let guard = hs.store.lock().unwrap();
        let ds = guard.get(&handle).expect("handle present");

        let has_canonical = ds.columns.iter().any(|c| c.name == "Product Category");
        assert!(
            has_canonical,
            "Exposed column name must be canonical 'Product Category', got: {:?}",
            ds.columns.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
    }

    /// AC-5 (back-compat): The legacy raw-key resolver `clean_result_rows` still
    /// works for back-compat — callers can still pass a raw key and get back the
    /// canonical label.  (FR-6: resolver stays.)
    #[test]
    fn canonical_ac5_baccompat_resolver_still_works() {
        let cat_key = "product_dimension_x005b_Product_x0020_Category_x005d_";
        let bound = json!({
            "dimensions": [{ "unique_name": "product_dimension.[Product Category]" }],
            "measures": [],
        });
        let rows = vec![json!({ cat_key: "Books" })];

        // The resolver (clean_result_rows) maps a raw key to its canonical label.
        let cleaned = clean_result_rows(&rows, &bound);
        let row = cleaned[0].as_object().unwrap();
        assert!(
            row.contains_key("Product Category"),
            "Resolver must map '{}' → 'Product Category'; got: {:?}",
            cat_key,
            row.keys().collect::<Vec<_>>()
        );
    }

    /// AC-6 (idempotent): Re-persisting an already-canonical result is a no-op on names.
    ///
    /// `clean_result_rows(clean_rows, bound)` == `clean_rows` — normalization is
    /// idempotent (FR-7).
    #[test]
    fn canonical_ac6_idempotent_already_clean() {
        let bound = json!({
            "dimensions": [{ "unique_name": "product_dimension.[Product Category]" }],
            "measures": [{ "unique_name": "sales.Revenue" }],
        });
        // Already-canonical rows (as produced by the response path):
        let clean_rows = vec![
            json!({ "Product Category": "Books", "Revenue": 100.0 }),
        ];

        // Re-applying clean_result_rows is a no-op on names.
        let re_cleaned = clean_result_rows(&clean_rows, &bound);
        let re_row = re_cleaned[0].as_object().unwrap();
        assert!(
            re_row.contains_key("Product Category"),
            "Idempotent: 'Product Category' must survive re-cleaning; got: {:?}",
            re_row.keys().collect::<Vec<_>>()
        );
        assert!(
            re_row.contains_key("Revenue"),
            "Idempotent: 'Revenue' must survive re-cleaning; got: {:?}",
            re_row.keys().collect::<Vec<_>>()
        );
        // No spurious new keys.
        assert_eq!(re_row.len(), 2);
    }

    /// AC-7 (shared fn): The response builder and persist path produce identical
    /// column names from the same raw keys + bound.  One function, no fork.
    #[test]
    fn canonical_ac7_shared_function_response_and_persist_identical() {
        let year_key = "sold_date_dimensions_x005b_Sold_x0020_Calendar_x0020_Year_x005d_";
        let sales_key = "_x005b_Total_x0020_Store_x0020_Sales_x005d_";
        let bound = json!({
            "dimensions": [{ "unique_name": "sold_date_dimensions.[Sold Calendar Year]" }],
            "measures": [{ "unique_name": "tpcds.total_store_sales" }],
        });
        let rows = vec![
            json!({ year_key: 2021.0, sales_key: 100.5 }),
            json!({ year_key: 2022.0, sales_key: 200.5 }),
        ];

        // Response path: clean_result_rows produces the canonical names.
        let response_cols: Vec<String> = clean_result_rows(&rows, &bound)
            .first()
            .and_then(Value::as_object)
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default();

        // Persist path: put_rows_with_canonical_labels uses the SAME clean_result_rows.
        use crate::handle_ops::HandleStore;
        let hs = HandleStore::new();
        let handle = hs
            .put_rows_with_canonical_labels(&rows, &bound)
            .expect("put succeeds");
        let guard = hs.store.lock().unwrap();
        let ds = guard.get(&handle).expect("handle present");
        let handle_cols: Vec<&str> = ds.columns.iter().map(|c| c.name.as_str()).collect();

        // Both must have the same column names.
        assert_eq!(
            response_cols.len(),
            handle_cols.len(),
            "Column count mismatch: response {response_cols:?} vs handle {handle_cols:?}"
        );
        for col in &response_cols {
            assert!(
                handle_cols.contains(&col.as_str()),
                "Handle missing response column '{col}'; handle: {handle_cols:?}"
            );
        }
    }

    // ── PRD-mqo-dataset-op-clean-args AC tests (v0.33.0) ────────────────────

    /// Fixture that stores a canonical-label handle for testing dataset_* ops.
    ///
    /// Returns `(SharedStore, DatasetHandle)` with columns `["Year", "Revenue"]`.
    /// `Year` is a Dimension, `Revenue` is a Measure (bound-authoritative).
    fn fixture_year_revenue_handle() -> (SharedStore, DatasetHandle) {
        let year_key = "atscale_catalogs_x005b_Sold_x0020_Calendar_x0020_Year_x005d_";
        let revenue_key = "_x005b_Revenue_x005d_";
        let bound = json!({
            "dimensions": [{ "unique_name": "sold_date.[Sold Calendar Year]", "label": "Year" }],
            "measures": [{ "unique_name": "sales.Revenue", "label": "Revenue" }],
        });
        let rows = vec![
            json!({ year_key: 2021.0, revenue_key: 100.0 }),
            json!({ year_key: 2022.0, revenue_key: 200.0 }),
            json!({ year_key: 2023.0, revenue_key: 150.0 }),
        ];
        let hs = HandleStore::new();
        let handle = hs
            .put_rows_with_canonical_labels(&rows, &bound)
            .expect("put succeeds");
        (hs.store.clone(), handle)
    }

    /// AC-1 (sort): `dataset_sort` with canonical column name `"Revenue"` works
    /// directly — no resolver needed.  Confirms FR-1 for sort.
    #[test]
    fn clean_args_ac1_sort_by_canonical_revenue() {
        let (store, handle) = fixture_year_revenue_handle();
        let result = handle_dataset_sort(
            &store,
            &json!({
                "handle": handle.id,
                "keys": [{ "col": "Revenue", "dir": "desc" }]
            }),
            25,
        );
        let is_err = result["isError"].as_bool().unwrap_or(true);
        assert!(!is_err, "dataset_sort with canonical 'Revenue' must succeed; got: {result}");
        // Top row after desc sort should be year 2022 (Revenue=200).
        if let Some(rows) = result["structuredContent"]["rows"].as_array() {
            let top_rev = rows[0]["Revenue"].as_f64().unwrap_or(0.0);
            assert!(
                (top_rev - 200.0).abs() < 1e-6,
                "After desc sort, first row Revenue must be 200.0, got {top_rev}"
            );
        }
    }

    /// AC-2 (filter): `dataset_filter` with canonical column name `"Year"` works
    /// directly — confirms FR-1 for filter.
    #[test]
    fn clean_args_ac2_filter_by_canonical_year() {
        let (store, handle) = fixture_year_revenue_handle();
        let result = handle_dataset_filter(
            &store,
            &json!({
                "handle": handle.id,
                "predicate": { "col": "Year", "op": "eq", "val": 2022.0 }
            }),
            25,
        );
        let is_err = result["isError"].as_bool().unwrap_or(true);
        assert!(!is_err, "dataset_filter with canonical 'Year' must succeed; got: {result}");
        let row_count = result["structuredContent"]["row_count"].as_u64().unwrap_or(0);
        assert_eq!(row_count, 1, "Filter Year=2022 must return exactly 1 row");
    }

    /// AC-3 (round-trip invariant, FR-3): Every column name from a
    /// `query_multidimensional`-style response (produced by `clean_result_rows`)
    /// is accepted unchanged by every `dataset_*` op.
    ///
    /// This is the guardrail that would have caught the chart-path regression
    /// (AC10 in the eval): column names from the response are used verbatim as
    /// op args, and every op must accept them.
    ///
    /// Fixture: mangled raw rows → `clean_result_rows` → columns `"Year"` and
    /// `"Revenue"` → stored via `put_rows_with_canonical_labels`.  Then every op
    /// that takes a column arg is exercised with both column names.
    #[test]
    fn clean_args_ac3_round_trip_invariant_every_op_accepts_response_columns() {
        // Step 1: simulate a query response — raw XMLA-mangled rows.
        let year_key = "atscale_catalogs_x005b_Sold_x0020_Calendar_x0020_Year_x005d_";
        let revenue_key = "_x005b_Revenue_x005d_";
        let bound = json!({
            "dimensions": [{ "unique_name": "sold_date.[Sold Calendar Year]", "label": "Year" }],
            "measures": [{ "unique_name": "sales.Revenue", "label": "Revenue" }],
        });
        let raw_rows = vec![
            json!({ year_key: 2021.0, revenue_key: 100.0 }),
            json!({ year_key: 2022.0, revenue_key: 200.0 }),
            json!({ year_key: 2023.0, revenue_key: 150.0 }),
        ];

        // Step 2: the response path produces the column names the LLM sees.
        let response_rows = clean_result_rows(&raw_rows, &bound);
        let response_cols: Vec<String> = response_rows
            .first()
            .and_then(Value::as_object)
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default();
        // Must be ["Year", "Revenue"] (or in some order).
        assert!(
            response_cols.contains(&"Year".to_string()),
            "Response must contain 'Year', got: {response_cols:?}"
        );
        assert!(
            response_cols.contains(&"Revenue".to_string()),
            "Response must contain 'Revenue', got: {response_cols:?}"
        );

        // Step 3: store via the canonical path (same as the real query pipeline).
        let hs = HandleStore::new();
        let handle = hs
            .put_rows_with_canonical_labels(&raw_rows, &bound)
            .expect("put succeeds");
        let store = hs.store.clone();

        // Step 4: assert every op accepts the response column names unchanged.
        //
        // dataset_sort
        let r = handle_dataset_sort(
            &store,
            &json!({ "handle": handle.id, "keys": [{ "col": "Revenue", "dir": "desc" }] }),
            25,
        );
        assert!(!r["isError"].as_bool().unwrap_or(true), "round-trip: dataset_sort(Revenue) failed: {r}");

        // dataset_filter
        let r = handle_dataset_filter(
            &store,
            &json!({ "handle": handle.id, "predicate": { "col": "Year", "op": "gt", "val": 2020.0 } }),
            25,
        );
        assert!(!r["isError"].as_bool().unwrap_or(true), "round-trip: dataset_filter(Year) failed: {r}");

        // dataset_aggregate (group_by Year, sum Revenue)
        let r = handle_dataset_aggregate(
            &store,
            &json!({ "handle": handle.id, "group_by": ["Year"], "agg": "sum", "measure": "Revenue" }),
            25,
            None, // no catalog in test fixture — guard is catalog-absent fail-open
        );
        assert!(!r["isError"].as_bool().unwrap_or(true), "round-trip: dataset_aggregate(Year,Revenue) failed: {r}");

        // dataset_top_n (by Revenue)
        let r = handle_dataset_top_n(
            &store,
            &json!({ "handle": handle.id, "n": 2, "measure": "Revenue" }),
            25,
        );
        assert!(!r["isError"].as_bool().unwrap_or(true), "round-trip: dataset_top_n(Revenue) failed: {r}");

        // dataset_slice (filter by Year)
        let r = handle_dataset_slice(
            &store,
            &json!({ "handle": handle.id, "filters": [{ "col": "Year", "op": "=", "value": 2022.0 }] }),
            25,
        );
        assert!(!r["isError"].as_bool().unwrap_or(true), "round-trip: dataset_slice(Year) failed: {r}");

        // dataset_period_over_period (date_col=Year, measure_cols=[Revenue])
        let r = handle_dataset_period_over_period(
            &store,
            &json!({ "handle": handle.id, "date_col": "Year", "period": "year", "measure_cols": ["Revenue"] }),
            25,
        );
        assert!(!r["isError"].as_bool().unwrap_or(true), "round-trip: dataset_period_over_period(Year,Revenue) failed: {r}");

        // dataset_chart (x_col=Year, y_cols=[Revenue])
        let r = handle_dataset_chart(
            &store,
            &json!({ "handle": handle.id, "chart_type": "bar", "x_col": "Year", "y_cols": ["Revenue"] }),
            25,
        );
        assert!(!r["isError"].as_bool().unwrap_or(true), "round-trip: dataset_chart(Year,Revenue) failed: {r}");

        // dataset_export (no column args; just verify it works)
        let r = handle_dataset_export(&store, &json!({ "handle": handle.id, "format": "json" }));
        assert!(!r["isError"].as_bool().unwrap_or(true), "round-trip: dataset_export failed: {r}");
        // Export columns must match response columns.
        if let Some(rows) = r["structuredContent"]["rows"].as_array() {
            if let Some(first) = rows.first().and_then(Value::as_object) {
                let export_cols: Vec<&str> = first.keys().map(String::as_str).collect();
                for col in &response_cols {
                    assert!(
                        export_cols.contains(&col.as_str()),
                        "dataset_export must return response column '{col}'; export has: {export_cols:?}"
                    );
                }
            }
        }
    }

    /// AC-4 (unknown column, FR-5): An unrecognised column arg returns a typed
    /// `unknown_column` error that lists the handle's actual canonical columns.
    #[test]
    fn clean_args_ac4_unknown_column_returns_typed_error_with_column_list() {
        let (store, handle) = fixture_year_revenue_handle();
        let result = handle_dataset_sort(
            &store,
            &json!({
                "handle": handle.id,
                "keys": [{ "col": "NonExistentColumn", "dir": "asc" }]
            }),
            25,
        );
        let is_err = result["isError"].as_bool().unwrap_or(false);
        assert!(is_err, "Unknown column must produce an error; got: {result}");

        let sc = &result["structuredContent"];
        let code = sc["error"]["code"].as_str().unwrap_or("");
        assert_eq!(code, "unknown_column", "Error code must be 'unknown_column'; got: {sc}");

        // The error must include the available columns list (FR-5).
        let available = &sc["error"]["available_columns"];
        assert!(
            available.is_array(),
            "Error must include 'available_columns' array; got: {sc}"
        );
        let cols: Vec<&str> = available
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            cols.contains(&"Year") && cols.contains(&"Revenue"),
            "available_columns must list 'Year' and 'Revenue'; got: {cols:?}"
        );
    }

    /// AC-5 (back-compat, FR-4): A legacy raw XMLA key passed as a column arg
    /// resolves via the back-compat resolver with a deprecation warning, and the
    /// op succeeds with correct results.
    #[test]
    fn clean_args_ac5_legacy_raw_key_resolves_via_fallback_with_warning() {
        let (store, handle) = fixture_year_revenue_handle();
        // "_x005b_Revenue_x005d_" is the legacy raw key that clean_label maps to "Revenue".
        let legacy_key = "_x005b_Revenue_x005d_";
        let result = handle_dataset_sort(
            &store,
            &json!({
                "handle": handle.id,
                "keys": [{ "col": legacy_key, "dir": "desc" }]
            }),
            25,
        );
        let is_err = result["isError"].as_bool().unwrap_or(true);
        assert!(
            !is_err,
            "Legacy raw key '{}' must resolve via back-compat fallback; got: {result}",
            legacy_key
        );
        // A deprecation warning must be present in the response.
        let warnings = &result["structuredContent"]["warnings"];
        assert!(
            warnings.is_array() && !warnings.as_array().unwrap().is_empty(),
            "Back-compat resolver must include a deprecation warning; got: {result}"
        );
        let warn_str = warnings.as_array().unwrap()[0].as_str().unwrap_or("");
        assert!(
            warn_str.contains("DEPRECATION"),
            "Warning must contain 'DEPRECATION'; got: '{warn_str}'"
        );
    }

    /// AC-6 (semantics, FR-6): Op results are semantically identical when using
    /// the canonical name vs the legacy key (both must sort the same way).
    #[test]
    fn clean_args_ac6_semantics_unchanged_canonical_vs_legacy() {
        let (store, handle) = fixture_year_revenue_handle();
        let legacy_key = "_x005b_Revenue_x005d_";

        // Sort by canonical name.
        let r_canonical = handle_dataset_sort(
            &store,
            &json!({ "handle": handle.id, "keys": [{ "col": "Revenue", "dir": "asc" }] }),
            25,
        );
        // Sort by legacy key.
        let r_legacy = handle_dataset_sort(
            &store,
            &json!({ "handle": handle.id, "keys": [{ "col": legacy_key, "dir": "asc" }] }),
            25,
        );

        assert!(!r_canonical["isError"].as_bool().unwrap_or(true), "canonical sort failed: {r_canonical}");
        assert!(!r_legacy["isError"].as_bool().unwrap_or(true), "legacy sort failed: {r_legacy}");

        // Both must produce the same row_count and same Revenue values in order.
        let rc = r_canonical["structuredContent"]["row_count"].as_u64().unwrap_or(0);
        let rl = r_legacy["structuredContent"]["row_count"].as_u64().unwrap_or(0);
        assert_eq!(rc, rl, "row_count must be identical: canonical={rc} legacy={rl}");

        if let (Some(rows_c), Some(rows_l)) = (
            r_canonical["structuredContent"]["rows"].as_array(),
            r_legacy["structuredContent"]["rows"].as_array(),
        ) {
            for (i, (rc_row, rl_row)) in rows_c.iter().zip(rows_l.iter()).enumerate() {
                let rc_rev = rc_row["Revenue"].as_f64().unwrap_or(f64::NAN);
                let rl_rev = rl_row["Revenue"].as_f64().unwrap_or(f64::NAN);
                assert!(
                    (rc_rev - rl_rev).abs() < 1e-6,
                    "Row {i}: Revenue mismatch canonical={rc_rev} legacy={rl_rev}"
                );
            }
        }
    }

    /// `resolve_col_name` unit tests: exact match, clean_label match, case-insensitive.
    #[test]
    fn resolve_col_name_exact_match_is_not_legacy() {
        let ds = json_rows_to_dataset(&[json!({ "Year": 2021.0, "Revenue": 100.0 })]);
        let r = resolve_col_name("Year", &ds);
        assert_eq!(r, Some(("Year", false)), "Exact match should not be legacy");
    }

    #[test]
    fn resolve_col_name_clean_label_match_is_legacy() {
        let ds = json_rows_to_dataset(&[json!({ "Revenue": 100.0 })]);
        // "_x005b_Revenue_x005d_" decodes to "Revenue" via clean_label.
        let r = resolve_col_name("_x005b_Revenue_x005d_", &ds);
        assert_eq!(r, Some(("Revenue", true)), "clean_label match should be legacy");
    }

    #[test]
    fn resolve_col_name_no_match_returns_none() {
        let ds = json_rows_to_dataset(&[json!({ "Revenue": 100.0 })]);
        assert_eq!(resolve_col_name("NonExistent", &ds), None);
    }

    #[test]
    fn replace_string_in_value_replaces_nested() {
        let v = json!({ "predicate": { "col": "old_name", "val": 5 }, "keys": ["old_name"] });
        let replaced = replace_string_in_value(v, "old_name", "new_name");
        assert_eq!(replaced["predicate"]["col"].as_str(), Some("new_name"));
        assert_eq!(replaced["keys"][0].as_str(), Some("new_name"));
    }

    // ── Blank-member counting / captioning (PRD-mqo-blank-member-answer-fidelity) ──

    #[test]
    fn count_blank_dimension_member_rows_counts_only_dim_blanks() {
        // String column = dimension; numeric column = measure. A NULL category
        // counts; a NULL/empty-string category counts; a NULL measure does not.
        let rows = vec![
            json!({ "Product Category": "Electronics", "n": 100 }),
            json!({ "Product Category": serde_json::Value::Null, "n": 43 }),
            json!({ "Product Category": "   ", "n": 7 }),     // whitespace-only = blank
            json!({ "Product Category": "Books", "n": serde_json::Value::Null }), // measure null ≠ blank member
        ];
        let bound = json!({});
        assert_eq!(count_blank_dimension_member_rows(&rows, &bound), 2);
    }

    #[test]
    fn count_blank_dimension_member_rows_zero_when_all_present() {
        let rows = vec![
            json!({ "State": "TX", "sales": 9 }),
            json!({ "State": "CA", "sales": 8 }),
        ];
        assert_eq!(count_blank_dimension_member_rows(&rows, &json!({})), 0);
    }

    #[test]
    fn apply_blank_member_caption_rewrites_only_blank_dim_cells() {
        let mut rows = vec![
            json!({ "Product Category": "Electronics", "n": 100 }),
            json!({ "Product Category": serde_json::Value::Null, "n": 43 }),
        ];
        apply_blank_member_caption(&mut rows, &json!({}), "(blank)");
        assert_eq!(rows[0]["Product Category"], json!("Electronics"));
        assert_eq!(rows[1]["Product Category"], json!("(blank)"));
        assert_eq!(rows[1]["n"], json!(43), "measure cell untouched");
    }
}
