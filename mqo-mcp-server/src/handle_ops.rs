//! Handle-operation MCP tools: `dataset_aggregate`, `dataset_slice`,
//! `dataset_period_over_period`, `dataset_chart`.
//!
//! Each tool operates over an in-memory result store (immutable derive-new
//! semantics: input handle is never mutated; a new handle is minted for every
//! derived result).  All computation is pure Rust over [`serde_json::Value`] rows
//! — no `AtScale` engine round-trip is issued after the initial
//! `query_multidimensional` call.
//!
//! **Inline threshold**: [`INLINE_THRESHOLD`] controls the maximum number of
//! raw rows included in any `head_sample`.  When a result's `row_count` exceeds
//! this constant, the summary carries at most K rows and no full row dump is
//! ever emitted.

use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use mqo_duckdb_handle_store::{ColumnSchema, DatasetHandle, MemStore, ResultStore};
use serde_json::{json, Value};

/// Maximum number of raw rows to include in any `head_sample` or inline
/// `data.values` payload.  Shared across all four handle-op tools (R11).
pub const INLINE_THRESHOLD: usize = 20;

// ── Store accessor type ───────────────────────────────────────────────────────

/// A shared, locked [`MemStore`].  Wrapped in `Arc<Mutex<…>>` inside [`HandleStore`].
pub type SharedStore = std::sync::Arc<std::sync::Mutex<MemStore>>;

/// Public wrapper that owns the shared store and exposes the four tool handlers.
pub struct HandleStore {
    pub store: SharedStore,
}

impl HandleStore {
    /// Create a new [`HandleStore`] backed by a freshly-allocated [`MemStore`].
    #[must_use]
    pub fn new() -> Self {
        HandleStore {
            store: std::sync::Arc::new(std::sync::Mutex::new(
                mqo_duckdb_handle_store::MemStore::with_defaults(),
            )),
        }
    }
}

impl Default for HandleStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Wall-clock seconds since epoch.
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Infer a `Vec<ColumnSchema>` from the first row's keys.
fn infer_schema(rows: &[Value]) -> Vec<ColumnSchema> {
    let Some(first) = rows.first() else {
        return vec![];
    };
    let Some(obj) = first.as_object() else {
        return vec![];
    };
    obj.keys()
        .map(|k| ColumnSchema {
            name: k.clone(),
            ty: infer_type(first.get(k).unwrap_or(&Value::Null)),
        })
        .collect()
}

/// Heuristic type inference for a JSON value.
fn infer_type(v: &Value) -> String {
    match v {
        Value::Number(n) if n.is_f64() => "double".to_string(),
        Value::Number(_) => "integer".to_string(),
        Value::Bool(_) => "boolean".to_string(),
        _ => "string".to_string(),
    }
}

/// Build the standard `{new_handle, row_count, schema, head_sample}` result envelope.
#[must_use]
fn handle_result(handle: &DatasetHandle, row_count: usize, schema: &[ColumnSchema], head: &[Value]) -> Value {
    let schema_json: Vec<Value> = schema
        .iter()
        .map(|c| json!({ "name": c.name, "ty": c.ty }))
        .collect();
    json!({
        "new_handle": handle.0,
        "row_count": row_count,
        "schema": schema_json,
        "head_sample": head
    })
}

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

/// Retrieve all rows for a handle from the store.
fn get_all_rows(store: &MemStore, handle: &DatasetHandle) -> Result<(Vec<Value>, Vec<ColumnSchema>), String> {
    let meta = store.metadata(handle).map_err(|e| e.to_string())?;
    let rows = store
        .get_rows(handle, 0, meta.row_count)
        .map_err(|e| e.to_string())?;
    Ok((rows, meta.schema))
}

/// Parse a handle string from `args["handle"]` into a [`DatasetHandle`].
fn parse_handle(args: &Value) -> Result<DatasetHandle, Value> {
    let Some(s) = args.get("handle").and_then(Value::as_str) else {
        return Err(handle_err("invalid_params", "missing required field 'handle'"));
    };
    Ok(DatasetHandle(s.to_string()))
}

// ── Filter evaluation ─────────────────────────────────────────────────────────

/// Apply a single filter `{col, op, value}` against a JSON row.
/// Returns `true` if the row satisfies the filter.
fn eval_filter(row: &Value, filter: &Value) -> bool {
    let Some(col) = filter.get("col").and_then(Value::as_str) else {
        return true; // malformed filter → pass-through
    };
    let op = filter.get("op").and_then(Value::as_str).unwrap_or("=");
    let fv = filter.get("value").unwrap_or(&Value::Null);
    let rv = row.get(col).unwrap_or(&Value::Null);

    if op == "in" {
        let Some(arr) = fv.as_array() else {
            return false;
        };
        return arr.iter().any(|v| values_eq(rv, v));
    }

    // Attempt numeric comparison first; fall back to string.
    let num_cmp: Option<std::cmp::Ordering> = if let (Some(rf), Some(ff)) = (rv.as_f64(), fv.as_f64()) {
        rf.partial_cmp(&ff)
    } else {
        let rs = rv.as_str().unwrap_or("");
        let fs = fv.as_str().unwrap_or("");
        Some(rs.cmp(fs))
    };

    match (op, num_cmp) {
        ("=" | "==", Some(o)) => o == std::cmp::Ordering::Equal,
        ("!=" | "<>", Some(o)) => o != std::cmp::Ordering::Equal,
        ("<", Some(o)) => o == std::cmp::Ordering::Less,
        ("<=", Some(o)) => matches!(o, std::cmp::Ordering::Less | std::cmp::Ordering::Equal),
        (">", Some(o)) => o == std::cmp::Ordering::Greater,
        (">=", Some(o)) => matches!(o, std::cmp::Ordering::Greater | std::cmp::Ordering::Equal),
        _ => true,
    }
}

/// Equality check that compares JSON values semantically.
fn values_eq(a: &Value, b: &Value) -> bool {
    if let (Some(af), Some(bf)) = (a.as_f64(), b.as_f64()) {
        return (af - bf).abs() < f64::EPSILON;
    }
    a == b
}

/// Apply all filters from the `filters` slice; returns `true` if all pass.
fn eval_filters(row: &Value, filters: &[Value]) -> bool {
    filters.iter().all(|f| eval_filter(row, f))
}

// ── Aggregation ───────────────────────────────────────────────────────────────

/// Supported aggregation functions.
#[derive(Debug, Clone)]
enum Agg {
    Sum,
    Avg,
    Min,
    Max,
    Count,
    CountDistinct,
}

impl Agg {
    fn from_str(s: &str) -> Option<Agg> {
        match s.to_lowercase().as_str() {
            "sum" => Some(Agg::Sum),
            "avg" | "mean" => Some(Agg::Avg),
            "min" => Some(Agg::Min),
            "max" => Some(Agg::Max),
            "count" => Some(Agg::Count),
            "count_distinct" => Some(Agg::CountDistinct),
            _ => None,
        }
    }
}

/// Aggregate a column over a slice of row references using the given function.
fn agg_col(rows: &[&Value], col: &str, agg: &Agg) -> Value {
    match agg {
        Agg::Count => Value::from(rows.len()),
        Agg::CountDistinct => {
            let mut seen = std::collections::HashSet::new();
            for row in rows {
                if let Some(v) = row.get(col) {
                    seen.insert(v.to_string());
                }
            }
            Value::from(seen.len())
        }
        Agg::Sum => {
            let total: f64 = rows
                .iter()
                .filter_map(|r| r.get(col).and_then(Value::as_f64))
                .sum();
            Value::from(total)
        }
        Agg::Avg => {
            let vals: Vec<f64> = rows
                .iter()
                .filter_map(|r| r.get(col).and_then(Value::as_f64))
                .collect();
            if vals.is_empty() {
                Value::Null
            } else {
                #[allow(clippy::cast_precision_loss)]
                let mean = vals.iter().sum::<f64>() / (vals.len() as f64);
                Value::from(mean)
            }
        }
        Agg::Min => rows
            .iter()
            .filter_map(|r| r.get(col).and_then(Value::as_f64))
            .reduce(f64::min)
            .map_or(Value::Null, Value::from),
        Agg::Max => rows
            .iter()
            .filter_map(|r| r.get(col).and_then(Value::as_f64))
            .reduce(f64::max)
            .map_or(Value::Null, Value::from),
    }
}

// ── Aggregate helper (factored out for line-count) ────────────────────────────

/// Parse measure specs from args — returns `Err(error_value)` on bad input.
fn parse_measure_specs(args: &Value) -> Result<Vec<(String, Agg)>, Value> {
    let measures_raw: Vec<&Value> = args
        .get("measures")
        .and_then(Value::as_array)
        .map_or(vec![], |a| a.iter().collect());

    if measures_raw.is_empty() {
        return Err(handle_err("invalid_params", "measures must be a non-empty array"));
    }

    let mut specs = Vec::new();
    for m in &measures_raw {
        let Some(col) = m.get("col").and_then(Value::as_str) else {
            return Err(handle_err("invalid_params", "each measure must have a 'col' field"));
        };
        let agg_str = m.get("agg").and_then(Value::as_str).unwrap_or("sum");
        let Some(agg) = Agg::from_str(agg_str) else {
            return Err(handle_err("invalid_params", &format!("unsupported agg '{agg_str}'; valid: sum, avg, min, max, count, count_distinct")));
        };
        specs.push((col.to_string(), agg));
    }
    Ok(specs)
}

/// Group rows and compute aggregations — returns the sorted result rows.
fn compute_aggregate(
    rows: &[Value],
    group_by: &[String],
    measure_specs: &[(String, Agg)],
    filters: &[Value],
) -> Vec<Value> {
    // Pre-filter.
    let filtered: Vec<&Value> = rows.iter().filter(|r| eval_filters(r, filters)).collect();

    // Group by key = null-separated group column values.
    let mut groups: HashMap<String, Vec<&Value>> = HashMap::new();
    let mut group_key_rows: HashMap<String, Value> = HashMap::new();

    for row in &filtered {
        let key: String = group_by
            .iter()
            .map(|col| row.get(col).map_or_else(String::new, std::string::ToString::to_string))
            .collect::<Vec<_>>()
            .join("\x00");

        group_key_rows.entry(key.clone()).or_insert_with(|| {
            let mut gb_map = serde_json::Map::new();
            for col in group_by {
                gb_map.insert(col.clone(), row.get(col).cloned().unwrap_or(Value::Null));
            }
            Value::Object(gb_map)
        });
        groups.entry(key).or_default().push(row);
    }

    // Produce output rows: one per group.
    let mut result_rows: Vec<Value> = groups
        .iter()
        .map(|(key, group_rows)| {
            let mut out = group_key_rows[key].as_object().cloned().unwrap_or_default();
            for (col, agg) in measure_specs {
                let agg_key = format!("{col}_{}", format!("{agg:?}").to_lowercase());
                out.insert(agg_key, agg_col(group_rows, col, agg));
            }
            Value::Object(out)
        })
        .collect();

    // Sort deterministically by group_by values.
    result_rows.sort_by(|a, b| {
        let ka: String = group_by
            .iter()
            .map(|c| a.get(c).map_or_else(String::new, std::string::ToString::to_string))
            .collect();
        let kb: String = group_by
            .iter()
            .map(|c| b.get(c).map_or_else(String::new, std::string::ToString::to_string))
            .collect();
        ka.cmp(&kb)
    });

    result_rows
}

// ── Public tool handlers ──────────────────────────────────────────────────────

/// Handle the `dataset_aggregate` MCP tool.
///
/// Groups the input result by `group_by` columns, applies `measures` aggregations,
/// optionally pre-filters via `filters`, persists the result as a new handle,
/// and returns `{new_handle, row_count, schema, head_sample}`.
pub fn handle_dataset_aggregate(store: &SharedStore, args: &Value) -> Value {
    let handle = match parse_handle(args) {
        Ok(h) => h,
        Err(e) => return e,
    };

    let group_by: Vec<String> = args
        .get("group_by")
        .and_then(Value::as_array)
        .map_or(vec![], |a| {
            a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
        });

    if group_by.is_empty() {
        return handle_err("invalid_params", "group_by must be a non-empty array of column names");
    }

    let measure_specs = match parse_measure_specs(args) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let filters: Vec<Value> = args
        .get("filters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let Ok(mut store_guard) = store.lock() else {
        return handle_err("store_error", "store lock poisoned");
    };
    let (all_rows, _) = match get_all_rows(&store_guard, &handle) {
        Ok(r) => r,
        Err(e) => return handle_err("handle_not_found", &e),
    };

    let result_rows = compute_aggregate(&all_rows, &group_by, &measure_specs, &filters);
    let schema = infer_schema(&result_rows);
    let row_count = result_rows.len();
    let head: Vec<Value> = result_rows.iter().take(INLINE_THRESHOLD).cloned().collect();

    let envelope = match store_guard.put(&result_rows, &schema, unix_now()) {
        Ok(e) => e,
        Err(e) => return handle_err("store_error", &e.to_string()),
    };

    handle_ok(&handle_result(&envelope.handle, row_count, &schema, &head))
}

/// Handle the `dataset_slice` MCP tool.
///
/// Filters the input result to rows matching all `filters`, persists the result,
/// and returns `{new_handle, row_count, schema, head_sample}`.
/// An empty match is a valid result with `row_count` 0 (not an error).
pub fn handle_dataset_slice(store: &SharedStore, args: &Value) -> Value {
    let handle = match parse_handle(args) {
        Ok(h) => h,
        Err(e) => return e,
    };

    let filters: Vec<Value> = args
        .get("filters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let Ok(mut store_guard) = store.lock() else {
        return handle_err("store_error", "store lock poisoned");
    };
    let (all_rows, input_schema) = match get_all_rows(&store_guard, &handle) {
        Ok(r) => r,
        Err(e) => return handle_err("handle_not_found", &e),
    };

    let result_rows: Vec<Value> = all_rows
        .into_iter()
        .filter(|r| eval_filters(r, &filters))
        .collect();

    // For empty result, re-use the input schema so callers can see column names.
    let schema = if result_rows.is_empty() {
        input_schema
    } else {
        infer_schema(&result_rows)
    };
    let row_count = result_rows.len();
    let head: Vec<Value> = result_rows.iter().take(INLINE_THRESHOLD).cloned().collect();

    let envelope = match store_guard.put(&result_rows, &schema, unix_now()) {
        Ok(e) => e,
        Err(e) => return handle_err("store_error", &e.to_string()),
    };

    handle_ok(&handle_result(&envelope.handle, row_count, &schema, &head))
}

// ── Period-over-period helpers ────────────────────────────────────────────────

/// Bucket a date string by the given period specifier.
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

/// Build the period-over-period output rows from bucketed data.
fn build_pop_rows(
    bucket_rows: &BTreeMap<String, Vec<&Value>>,
    buckets: &[String],
    measure_cols: &[String],
) -> Vec<Value> {
    let mut result_rows = Vec::with_capacity(buckets.len());
    for (i, bucket) in buckets.iter().enumerate() {
        let rows = &bucket_rows[bucket];
        let mut out = serde_json::Map::new();
        out.insert("period_bucket".to_string(), Value::String(bucket.clone()));

        let mut current_vals: HashMap<&str, f64> = HashMap::new();
        for col in measure_cols {
            let total: f64 = rows
                .iter()
                .filter_map(|r| r.get(col.as_str()).and_then(Value::as_f64))
                .sum();
            out.insert(col.clone(), Value::from(total));
            current_vals.insert(col.as_str(), total);
        }

        if i > 0 {
            let prior_bucket = &buckets[i - 1];
            let prior_rows = &bucket_rows[prior_bucket];
            for col in measure_cols {
                let prior_total: f64 = prior_rows
                    .iter()
                    .filter_map(|r| r.get(col.as_str()).and_then(Value::as_f64))
                    .sum();
                let current = current_vals[col.as_str()];
                let delta = current - prior_total;
                let pct_delta = if prior_total.abs() > f64::EPSILON {
                    (delta / prior_total) * 100.0
                } else {
                    f64::NAN
                };
                out.insert(format!("{col}_prior"), Value::from(prior_total));
                out.insert(format!("{col}_delta"), Value::from(delta));
                out.insert(
                    format!("{col}_pct_delta"),
                    if pct_delta.is_nan() { Value::Null } else { Value::from(pct_delta) },
                );
            }
        } else {
            for col in measure_cols {
                out.insert(format!("{col}_prior"), Value::Null);
                out.insert(format!("{col}_delta"), Value::Null);
                out.insert(format!("{col}_pct_delta"), Value::Null);
            }
        }

        result_rows.push(Value::Object(out));
    }
    result_rows
}

/// Handle the `dataset_period_over_period` MCP tool.
///
/// Buckets `date_col` by `period`, aggregates `measure_cols` per bucket, and adds
/// LAG-style prior-period delta columns.
/// Returns `{new_handle, row_count, schema, head_sample}`.
pub fn handle_dataset_period_over_period(store: &SharedStore, args: &Value) -> Value {
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

    let Ok(mut store_guard) = store.lock() else {
        return handle_err("store_error", "store lock poisoned");
    };
    let (all_rows, _) = match get_all_rows(&store_guard, &handle) {
        Ok(r) => r,
        Err(e) => return handle_err("handle_not_found", &e),
    };

    // Group rows by bucket (BTreeMap → naturally sorted).
    let mut bucket_rows: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for row in &all_rows {
        let date_str = row.get(&date_col).and_then(Value::as_str).unwrap_or("");
        let bucket = bucket_date(date_str, period);
        bucket_rows.entry(bucket).or_default().push(row);
    }

    let buckets: Vec<String> = bucket_rows.keys().cloned().collect();
    let result_rows = build_pop_rows(&bucket_rows, &buckets, &measure_cols);
    let schema = infer_schema(&result_rows);
    let row_count = result_rows.len();
    let head: Vec<Value> = result_rows.iter().take(INLINE_THRESHOLD).cloned().collect();

    let envelope = match store_guard.put(&result_rows, &schema, unix_now()) {
        Ok(e) => e,
        Err(e) => return handle_err("store_error", &e.to_string()),
    };

    handle_ok(&handle_result(&envelope.handle, row_count, &schema, &head))
}

/// Handle the `dataset_chart` MCP tool.
///
/// Reads at most [`INLINE_THRESHOLD`] rows from the handle and emits a
/// Vega-Lite v5 JSON spec binding `x_col` and `y_cols` per the requested
/// `chart_type`.  Returns the spec directly — no new handle is created.
pub fn handle_dataset_chart(store: &SharedStore, args: &Value) -> Value {
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

    let Ok(store_guard) = store.lock() else {
        return handle_err("store_error", "store lock poisoned");
    };

    let rows = match store_guard.get_rows(&handle, 0, INLINE_THRESHOLD) {
        Ok(r) => r,
        Err(e) => return handle_err("handle_not_found", &e.to_string()),
    };

    let spec = build_vega_spec(vl_mark, &x_col, &y_cols, title, &rows);
    handle_ok(&spec)
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

// ── MCP tool descriptor helpers ───────────────────────────────────────────────

/// Return the four handle-op MCP tool descriptor objects for inclusion in
/// `tool_descriptors()`.
#[must_use]
pub fn handle_op_descriptors() -> Vec<Value> {
    vec![
        json!({
            "name": "dataset_aggregate",
            "description": "Aggregate a result-set handle by grouping on dimensions and rolling up measures (sum/avg/min/max/count). Computes server-side — no AtScale round-trip. Returns {new_handle, row_count, schema, head_sample}.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "UUID of the input result handle." },
                    "group_by": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Column names to group by."
                    },
                    "measures": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "col": { "type": "string" },
                                "agg": { "type": "string", "enum": ["sum","avg","min","max","count","count_distinct"] }
                            },
                            "required": ["col","agg"]
                        },
                        "description": "Aggregations to compute."
                    },
                    "filters": {
                        "type": "array",
                        "items": { "type": "object" },
                        "description": "Optional pre-aggregate row filters."
                    }
                },
                "required": ["handle","group_by","measures"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_slice",
            "description": "Filter a result-set handle to rows matching all supplied filters. Returns a new handle with the matching subset (row_count=0 for no matches — not an error). Computes server-side — no AtScale round-trip.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "UUID of the input result handle." },
                    "filters": {
                        "type": "array",
                        "items": { "type": "object" },
                        "description": "Row filters: [{col, op, value}] where op is =, !=, <, <=, >, >=, in."
                    }
                },
                "required": ["handle","filters"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_period_over_period",
            "description": "Compute period-over-period deltas by bucketing date_col by period, summing measure_cols per bucket, and adding prior-period value, absolute delta, and percentage delta columns. Computes server-side — no AtScale round-trip.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "UUID of the input result handle." },
                    "date_col": { "type": "string", "description": "Column containing date/timestamp values." },
                    "period": { "type": "string", "enum": ["day","week","month","quarter","year"], "description": "Bucketing period." },
                    "measure_cols": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Measure columns to aggregate and compare across periods."
                    }
                },
                "required": ["handle","date_col","period","measure_cols"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_chart",
            "description": "Produce a Vega-Lite v5 JSON spec from a result handle. Reads at most 20 rows for inline data.values. Returns the spec directly — no new handle is created. Computes server-side — no AtScale round-trip.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "UUID of the input result handle." },
                    "chart_type": { "type": "string", "enum": ["bar","line","area","point"], "description": "Vega-Lite mark type." },
                    "x_col": { "type": "string", "description": "Column to bind to the x-axis." },
                    "y_cols": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Column(s) to bind to the y-axis."
                    },
                    "title": { "type": "string", "description": "Optional chart title." }
                },
                "required": ["handle","chart_type","x_col","y_cols"],
                "additionalProperties": false
            },
            "annotations": { "readOnlyHint": true }
        }),
    ]
}
