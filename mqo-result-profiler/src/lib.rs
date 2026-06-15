//! `mqo-result-profiler` — typed column inventory from a `query_multidimensional`
//! response + catalog JSON.
//!
//! Takes a `query_multidimensional` response payload and a catalog JSON, and
//! produces a [`ResultProfile`] — a typed, serialisable column inventory that
//! downstream chart-emitters (chart recommender, Vega emitter, asset bundler)
//! consume instead of re-deriving column roles from raw rows.
//!
//! # Example
//! ```no_run
//! # use mqo_result_profiler::profile;
//! let response = serde_json::json!({
//!     "rows": [{"revenue": 100.0, "year": 2021}],
//!     "bound": {
//!         "measures": ["revenue"],
//!         "dimensions": ["year"]
//!     }
//! });
//! let catalog = serde_json::json!({
//!     "columns": [
//!         {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
//!         {"unique_name": "year", "label": "Year", "kind": "dimension",
//!          "hierarchy": "time.calendar"}
//!     ]
//! });
//! let p = profile(&response, &catalog).unwrap();
//! assert_eq!(p.row_count, 1);
//! ```

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// Role of a column in the result — derived from the `bound` object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// A numeric measure (e.g. revenue, quantity).
    Measure,
    /// A categorical or temporal dimension (e.g. region, year).
    Dimension,
}

/// Inferred data type of a column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataType {
    /// Numeric measure — always assigned to `Role::Measure` columns.
    Quantitative,
    /// Date/time dimension — from `time.*` catalog hierarchy or ISO-date value fallback.
    Temporal,
    /// Categorical string dimension.
    Nominal,
    /// Ordered categorical — reserved for v2; never emitted in v1.
    Ordinal,
}

/// Per-column statistics and metadata in a profiled result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnProfile {
    /// Field key as it appears in the `rows` objects.
    pub name: String,
    /// Human-readable label from the catalog (falls back to `name`).
    pub label: String,
    /// Column role: measure or dimension, derived from `bound`.
    pub role: Role,
    /// Inferred data type.
    pub data_type: DataType,
    /// Count of distinct non-null values observed in `rows`.
    pub cardinality: usize,
    /// Fraction of null values: `0.0..=1.0`.
    pub null_rate: f64,
    /// `(min, max)` over non-null numeric values; `None` for non-quantitative columns.
    pub measure_range: Option<(f64, f64)>,
    /// `true` when the catalog marks this column as a calculated field (e.g. `margin_pct`).
    pub is_calc: bool,
    /// `true` when the catalog carries a `semi_additive` block on this measure.
    pub semi_additive: bool,
}

/// Top-level profile of a `query_multidimensional` result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultProfile {
    /// One entry per projected column, in `bound` projection order.
    pub columns: Vec<ColumnProfile>,
    /// Total rows in the response.
    pub row_count: usize,
    /// Number of measure columns.
    pub measure_count: usize,
    /// Number of dimension columns.
    pub dimension_count: usize,
}

/// Errors that can occur during profiling.
#[derive(Debug, Error)]
pub enum ProfileError {
    /// The response JSON did not contain a `rows` array where expected.
    #[error("response payload missing 'rows' array")]
    MissingRows,
    /// The response JSON did not contain a `bound` object.
    #[error("response payload missing 'bound' object")]
    MissingBound,
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Extract the inner payload (rows + bound) from either:
/// - A direct `{rows: [...], bound: {...}}` object, or
/// - A nested MCP structuredContent envelope: `{structuredContent: [{json: {rows, bound}}]}`
fn extract_payload(response: &Value) -> Option<&Value> {
    // Direct shape
    if response.get("rows").is_some() && response.get("bound").is_some() {
        return Some(response);
    }
    // MCP structuredContent envelope: structuredContent[0].json
    if let Some(sc) = response.get("structuredContent") {
        if let Some(first) = sc.get(0) {
            if let Some(inner) = first.get("json") {
                if inner.get("rows").is_some() && inner.get("bound").is_some() {
                    return Some(inner);
                }
            }
        }
    }
    // content[0].text (text-encoded JSON)
    if let Some(content) = response.get("content") {
        if let Some(first) = content.get(0) {
            if let Some(text) = first.get("text").and_then(|v| v.as_str()) {
                // We can't parse here without allocation, but we signal to the
                // caller via None — they should pre-parse this layer.
                let _ = text;
            }
        }
    }
    None
}

/// Extract the list of projected column names from `bound`, in order:
/// measures first (in their listed order), then dimensions.
fn projection_order(bound: &Value) -> Vec<(String, Role)> {
    let mut cols: Vec<(String, Role)> = Vec::new();
    if let Some(measures) = bound.get("measures").and_then(|v| v.as_array()) {
        for m in measures {
            if let Some(name) = m.as_str() {
                cols.push((name.to_owned(), Role::Measure));
            }
        }
    }
    if let Some(dims) = bound.get("dimensions").and_then(|v| v.as_array()) {
        for d in dims {
            if let Some(name) = d.as_str() {
                cols.push((name.to_owned(), Role::Dimension));
            }
        }
    }
    cols
}

/// Derive a friendly label from a bound `unique_name` (e.g. `time.calendar.[Year]` → `Year`,
/// `Revenue` → `Revenue`).  Mirrors `handle_ops::label_from_unique_name` — kept local so the
/// profiler crate has no dependency on `mqo-mcp-server` internals.
fn label_from_unique_name(unique_name: &str) -> String {
    // Prefer the last `[...]` bracket segment (e.g. `time.calendar.[Year]` → `Year`).
    if let Some(open) = unique_name.rfind('[') {
        if let Some(close) = unique_name[open..].find(']') {
            let inner = &unique_name[open + 1..open + close];
            if !inner.is_empty() {
                return inner.to_owned();
            }
        }
    }
    // Fall back: last dot-segment, underscores → spaces.
    let tail = unique_name.rsplit('.').next().unwrap_or(unique_name);
    tail.replace('_', " ").trim().to_string()
}

/// Resolve a bound column name to the actual key present in `row_keys`.
///
/// Match priority (mirrors `clean_result_rows` in `handle_ops`):
/// 1. Exact key match (bound name == row key, fixture / simple-string path).
/// 2. `label_from_unique_name(bound_name)` == row key (clean-label path).
/// 3. Fall back to the bound name itself (even if absent from rows — caller will get nulls).
fn resolve_to_row_key(bound_name: &str, row_keys: &[String]) -> String {
    // Priority 1: exact match.
    if let Some(k) = row_keys.iter().find(|k| k.as_str() == bound_name) {
        return k.clone();
    }
    // Priority 2: friendly-label match.
    let label = label_from_unique_name(bound_name);
    if let Some(k) = row_keys.iter().find(|k| k.as_str() == label) {
        return k.clone();
    }
    // Fall back: return the bound name unchanged.
    bound_name.to_owned()
}

/// Look up a column entry in the catalog by `unique_name`.
fn catalog_entry<'c>(catalog: &'c Value, name: &str) -> Option<&'c Value> {
    let cols = catalog.get("columns").and_then(|v| v.as_array())?;
    cols.iter().find(|entry| {
        entry
            .get("unique_name")
            .and_then(|v| v.as_str()) == Some(name)
    })
}

/// Return the catalog label for a column, falling back to the field name.
fn catalog_label(entry: Option<&Value>, name: &str) -> String {
    entry
        .and_then(|e| e.get("label"))
        .and_then(|v| v.as_str())
        .unwrap_or(name)
        .to_owned()
}

/// Return true if the catalog hierarchy for this entry begins with `time.`.
fn is_temporal_hierarchy(entry: Option<&Value>) -> bool {
    entry
        .and_then(|e| e.get("hierarchy"))
        .and_then(|v| v.as_str())
        .is_some_and(|h| h.starts_with("time."))
}

/// Heuristic: does this string look like an ISO date / year?
/// Accepts: `YYYY`, `YYYY-MM-DD`, or any `YYYY-MM-DDT…` prefix.
fn looks_like_date(s: &str) -> bool {
    let s = s.trim();
    if s.len() == 4 {
        return s.chars().all(|c| c.is_ascii_digit());
    }
    if s.len() >= 10 {
        let bytes = s.as_bytes();
        return bytes[0..4].iter().all(u8::is_ascii_digit)
            && bytes[4] == b'-'
            && bytes[5..7].iter().all(u8::is_ascii_digit)
            && bytes[7] == b'-'
            && bytes[8..10].iter().all(u8::is_ascii_digit);
    }
    false
}

/// Determine the [`DataType`] for a dimension column.
/// Priority: catalog hierarchy → ISO-date fallback on string values.
fn dimension_data_type(entry: Option<&Value>, values: &[&Value]) -> DataType {
    if is_temporal_hierarchy(entry) {
        return DataType::Temporal;
    }
    // Fallback: all non-null string values look like dates?
    let string_values: Vec<&str> = values
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    if !string_values.is_empty() && string_values.iter().all(|s| looks_like_date(s)) {
        return DataType::Temporal;
    }
    DataType::Nominal
}

/// Compute per-column statistics from `rows`.
fn compute_stats(
    rows: &[Value],
    name: &str,
    role: &Role,
    entry: Option<&Value>,
) -> (usize, f64, Option<(f64, f64)>, DataType) {
    let total = rows.len();
    let mut null_count = 0usize;
    let mut seen_values: Vec<String> = Vec::new();
    let mut numeric_values: Vec<f64> = Vec::new();
    let mut raw_values: Vec<&Value> = Vec::new();

    for row in rows {
        let cell = row.get(name);
        match cell {
            None | Some(Value::Null) => {
                null_count += 1;
            }
            Some(v) => {
                raw_values.push(v);
                // Canonicalise for distinct count
                let key = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => v.to_string(),
                };
                if !seen_values.contains(&key) {
                    seen_values.push(key);
                }
                if let Some(n) = v.as_f64() {
                    numeric_values.push(n);
                }
            }
        }
    }

    let cardinality = seen_values.len();
    #[allow(clippy::cast_precision_loss)]
    let null_rate = if total == 0 {
        0.0
    } else {
        // usize→f64 precision loss is acceptable for null-rate (0–1 fraction).
        (null_count as f64) / (total as f64)
    };

    let data_type = match role {
        Role::Measure => DataType::Quantitative,
        Role::Dimension => dimension_data_type(entry, &raw_values),
    };

    let measure_range = if matches!(role, Role::Measure) && !numeric_values.is_empty() {
        let min = numeric_values
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min);
        let max = numeric_values
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        Some((min, max))
    } else {
        None
    };

    (cardinality, null_rate, measure_range, data_type)
}

// ── Public API ─────────────────────────────────────────────────────────────────

/// Profile a `query_multidimensional` response payload against a catalog JSON.
///
/// `response` may be either:
/// - A direct `{rows: [...], bound: {measures: [...], dimensions: [...]}}` object, or
/// - An MCP structuredContent envelope containing the above at `structuredContent[0].json`.
///
/// `catalog` must be `{columns: [{unique_name, label, kind, hierarchy?, is_calc?, semi_additive?}]}`.
///
/// # Errors
/// Returns [`ProfileError::MissingRows`] or [`ProfileError::MissingBound`] if
/// the response payload does not contain the expected fields.
pub fn profile(response: &Value, catalog: &Value) -> Result<ResultProfile, ProfileError> {
    let payload = extract_payload(response).ok_or(ProfileError::MissingRows)?;

    let rows_val = payload.get("rows").ok_or(ProfileError::MissingRows)?;
    let rows: &[Value] = rows_val
        .as_array()
        .ok_or(ProfileError::MissingRows)?
        .as_slice();

    let bound = payload.get("bound").ok_or(ProfileError::MissingBound)?;
    let projection = projection_order(bound);

    // Collect the actual column keys from the first row (the ground truth for what
    // field names the rows carry).  When the response went through the clean-label path
    // (v0.29.0+), these keys are clean semantic labels (e.g. `"Year"`, `"Revenue"`)
    // while the bound may carry raw unique_names (e.g. `"time.calendar.[Year]"`).
    // `resolve_to_row_key` maps each bound entry to its actual row key so that
    // `ColumnProfile::name` (and therefore the downstream encoding field) always
    // matches the rows the chart pipeline is handed.
    let row_keys: Vec<String> = rows
        .first()
        .and_then(Value::as_object)
        .map(|obj| obj.keys().cloned().collect())
        .unwrap_or_default();

    let mut columns: Vec<ColumnProfile> = Vec::with_capacity(projection.len());

    for (bound_name, role) in &projection {
        // Resolve to the actual row key (clean-label path: bound_name may be a raw
        // unique_name that does not appear in the rows).
        let row_key = resolve_to_row_key(bound_name, &row_keys);

        // Catalog lookup: try the raw bound name first (exact unique_name), then the
        // resolved row key (for catalogs keyed by clean label).
        let entry = catalog_entry(catalog, bound_name)
            .or_else(|| catalog_entry(catalog, &row_key));
        let label = catalog_label(entry, &row_key);

        let is_calc = entry
            .and_then(|e| e.get("is_calc"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let semi_additive = entry
            .and_then(|e| e.get("semi_additive"))
            .is_some_and(|v| !v.is_null());

        // Compute statistics using the resolved row key so that column values are
        // found correctly in the (clean-labelled) rows.
        let (cardinality, null_rate, measure_range, data_type) =
            compute_stats(rows, &row_key, role, entry);

        columns.push(ColumnProfile {
            name: row_key,
            label,
            role: role.clone(),
            data_type,
            cardinality,
            null_rate,
            measure_range,
            is_calc,
            semi_additive,
        });
    }

    let measure_count = columns.iter().filter(|c| c.role == Role::Measure).count();
    let dimension_count = columns.iter().filter(|c| c.role == Role::Dimension).count();

    Ok(ResultProfile {
        columns,
        row_count: rows.len(),
        measure_count,
        dimension_count,
    })
}
