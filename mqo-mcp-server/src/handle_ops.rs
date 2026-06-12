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

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use dh_spec::{ColumnRole, ColumnSchema, DatasetHandle, DType};
use dh_store::{ColumnData, Dataset, Store};
use dh_summary::{capabilities as dh_capabilities, summarize, SummaryCfg};
use serde_json::{json, Map, Value};

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
fn run_dh_op<F>(store: &SharedStore, args: &Value, inline_threshold: usize, op: F) -> Value
where
    F: FnOnce(&mut Store, &DatasetHandle, &Value) -> Result<dh_spec::OpResult, dh_ops::OpError>,
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
        Err(e) => op_err_to_envelope(&e),
    }
}

/// `dataset_aggregate` — group-by + aggregation via the dh-ops kernel.
pub fn handle_dataset_aggregate(store: &SharedStore, args: &Value, inline_threshold: usize) -> Value {
    let params = aggregate_args_to_params(args);
    run_dh_op(store, &params, inline_threshold, dh_ops::aggregate)
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
    handle_ok(&derived_payload(&new_handle, &out_ds, inline_threshold))
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
    let mut rows = dataset_to_json_rows(&ds);
    rows.truncate(inline_threshold);
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
            "description": "Aggregate a result-set handle by grouping on dimensions and rolling up a measure (sum/mean/min/max/count/count_distinct). Computes server-side over typed columns — no AtScale round-trip. Derives a new handle; returns {new_handle, row_count, summary, capabilities} (rows inlined only when row_count ≤ inline_threshold).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "group_by": { "type": "array", "items": { "type": "string" }, "description": "Column names to group by." },
                    "agg": { "type": "string", "enum": ["sum","mean","min","max","count","count_distinct"], "description": "Aggregation function." },
                    "measure": { "type": "string", "description": "Column to aggregate (not needed for count)." },
                    "measures": { "type": "array", "items": { "type": "object" }, "description": "Legacy multi-measure shape [{col,agg}]; first is used." }
                },
                "required": ["handle","group_by"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_filter",
            "description": "Filter a result-set handle by a compound AND/OR predicate over columns (ops: eq, ne, lt, le, gt, ge, in, contains, is_null, is_not_null). Computes server-side — no AtScale round-trip. Derives a new handle.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "predicate": { "type": "object", "description": "Predicate tree: {col,op,val} or {and:[…]} / {or:[…]}." }
                },
                "required": ["handle","predicate"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_sort",
            "description": "Sort a result-set handle by one or more keys (asc/desc, stable). Computes server-side — no AtScale round-trip. Derives a new handle.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "keys": { "type": "array", "items": { "type": "object", "properties": { "col": { "type": "string" }, "dir": { "type": "string", "enum": ["asc","desc"] } }, "required": ["col"] }, "description": "Sort keys in priority order." }
                },
                "required": ["handle","keys"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_top_n",
            "description": "Return the top or bottom N rows of a handle by a measure column (deterministic tie-break). Computes server-side — no AtScale round-trip. Derives a new handle.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "n": { "type": "integer", "description": "Number of rows to keep." },
                    "measure": { "type": "string", "description": "Measure column to rank by." },
                    "dir": { "type": "string", "enum": ["top","bottom"], "description": "Top (default) or bottom." }
                },
                "required": ["handle","n","measure"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_pivot",
            "description": "Pivot a handle's rows × columns × measure into a crosstab. Computes server-side — no AtScale round-trip. Derives a new handle.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "row_dim": { "type": "string", "description": "Column for pivot rows." },
                    "col_dim": { "type": "string", "description": "Column for pivot columns." },
                    "measure": { "type": "string", "description": "Measure to aggregate per cell." },
                    "agg": { "type": "string", "enum": ["sum","mean","min","max","count"], "description": "Cell aggregation (default sum)." }
                },
                "required": ["handle","row_dim","col_dim","measure"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_compare",
            "description": "Compare two handles by joining on keys and computing delta + pct-change for a measure. Computes server-side — no AtScale round-trip. Derives a new handle (multi-parent lineage).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the first (A) result." },
                    "handle_b": { "type": "object", "description": "The second result's full DatasetHandle JSON." },
                    "join_keys": { "type": "array", "items": { "type": "string" }, "description": "Columns to join on." },
                    "measure": { "type": "string", "description": "Measure column to diff." }
                },
                "required": ["handle","handle_b","join_keys","measure"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_drill",
            "description": "Expand a grouped row of a handle back to its constituent detail rows via lineage. Computes server-side — no AtScale round-trip. Derives a new handle.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the grouped result." },
                    "group_row": { "type": "object", "description": "Column→value map identifying the group to drill into." }
                },
                "required": ["handle","group_row"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_describe",
            "description": "Produce per-column stats (min/max/sum/mean/distinct) for a handle without changing rows. Computes server-side — no AtScale round-trip. Derives a new handle.",
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
            "description": "Filter a result-set handle to rows matching all supplied [{col, op, value}] filters (op: =, !=, <, <=, >, >=, in). Compatibility alias for dataset_filter. row_count=0 for no matches is not an error. Computes server-side — no AtScale round-trip. Derives a new handle.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "filters": { "type": "array", "items": { "type": "object" }, "description": "Row filters: [{col, op, value}]." }
                },
                "required": ["handle","filters"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_period_over_period",
            "description": "Compute period-over-period deltas by bucketing date_col by period, summing measure_cols per bucket, and adding prior-period value, absolute delta, and percentage delta columns. Computes server-side — no AtScale round-trip. Derives a new handle.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "date_col": { "type": "string", "description": "Column containing date/timestamp values." },
                    "period": { "type": "string", "enum": ["day","week","month","quarter","year"], "description": "Bucketing period." },
                    "measure_cols": { "type": "array", "items": { "type": "string" }, "description": "Measure columns to aggregate and compare across periods." }
                },
                "required": ["handle","date_col","period","measure_cols"]
            },
            "annotations": { "readOnlyHint": true }
        }),
        json!({
            "name": "dataset_chart",
            "description": "Produce a Vega-Lite v5 JSON spec from a result handle. Reads at most inline_threshold rows for inline data.values. Returns the spec directly — no new handle. Computes server-side — no AtScale round-trip.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "handle": { "type": "string", "description": "Handle id of the input result." },
                    "chart_type": { "type": "string", "enum": ["bar","line","area","point"], "description": "Vega-Lite mark type." },
                    "x_col": { "type": "string", "description": "Column to bind to the x-axis." },
                    "y_cols": { "type": "array", "items": { "type": "string" }, "description": "Column(s) to bind to the y-axis." },
                    "title": { "type": "string", "description": "Optional chart title." }
                },
                "required": ["handle","chart_type","x_col","y_cols"]
            },
            "annotations": { "readOnlyHint": true }
        }),
    ]
}
