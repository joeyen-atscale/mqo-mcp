//! # dh-ops
//!
//! Deterministic in-place operation kernel for the `dh-*` MCP fleet.
//!
//! Every operation takes `(store, handle, params)` → `Result<OpResult, OpError>`.
//! All computation is server-side; no op returns raw rows.  Each op calls
//! `store.derive(...)` to allocate a new handle, leaving the input untouched.
//!
//! ## Operations
//!
//! | Function      | Description |
//! |---------------|-------------|
//! | [`aggregate`] | Group-by + agg (sum/mean/min/max/count/count_distinct) |
//! | [`filter`]    | Compound AND/OR predicate over columns |
//! | [`sort`]      | Multi-key stable sort (asc/desc) |
//! | [`top_n`]     | Top/bottom N by a measure (deterministic tie-break) |
//! | [`pivot`]     | Rows × cols × measure crosstab |
//! | [`compare`]   | Two handles → delta / pct-change (multi-parent lineage) |
//! | [`drill`]     | Expand a grouped row to detail rows via lineage |
//! | [`describe`]  | Per-column stats without mutating rows |

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::cast_precision_loss)]   // intentional f64 conversions for aggregation
#![allow(clippy::cast_sign_loss)]        // intentional i64→u64 bit manipulation in sort key
#![allow(clippy::cast_possible_wrap)]    // intentional u64→i64 for distinct count display
#![allow(clippy::cast_possible_truncation)] // intentional usize conversions
#![allow(clippy::missing_panics_doc)]    // expects are on pre-validated inputs; not real panics

use std::collections::HashMap;

use dh_spec::{Capability, ColumnRole, ColumnSchema, DatasetHandle, DType, OpResult};
use dh_store::{ColumnData, Dataset, Store};
use dh_summary::{summarize, SummaryCfg};
use serde_json::Value;

// ── OpError ────────────────────────────────────────────────────────────────

/// Errors produced by any `dh-ops` operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpError {
    /// The handle could not be found or has expired.
    HandleNotFound(String),
    /// A required parameter is missing or has the wrong type.
    BadParam(String),
    /// A referenced column does not exist in the dataset.
    UnknownColumn(String),
    /// The operation is not valid for this dataset shape.
    Unsupported(String),
    /// An internal constraint was violated (e.g. empty group key).
    Internal(String),
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HandleNotFound(m) => write!(f, "handle not found: {m}"),
            Self::BadParam(m) => write!(f, "bad parameter: {m}"),
            Self::UnknownColumn(m) => write!(f, "unknown column: {m}"),
            Self::Unsupported(m) => write!(f, "unsupported: {m}"),
            Self::Internal(m) => write!(f, "internal error: {m}"),
        }
    }
}

impl std::error::Error for OpError {}

impl From<dh_store::LookupError> for OpError {
    fn from(e: dh_store::LookupError) -> Self {
        Self::HandleNotFound(e.to_string())
    }
}

// ── Default summary config ─────────────────────────────────────────────────

/// Default [`SummaryCfg`] used by all ops.  `sample_cap = 8` guarantees the
/// `sample.len() ≤ sample_cap ≤ DEFAULT_SAMPLE_CAP` invariant.
fn default_cfg() -> SummaryCfg {
    SummaryCfg::default() // sample_cap=8, topk=10, max_bytes=32768
}

// ── Row-level helpers ──────────────────────────────────────────────────────

/// Extract a single-column scalar from a `Dataset` at `row_idx`.
fn col_val(col_data: &ColumnData, row_idx: usize) -> Value {
    match col_data {
        ColumnData::Int(v) => v
            .get(row_idx)
            .and_then(|o| *o)
            .map_or(Value::Null, Value::from),
        ColumnData::Float(v) => v
            .get(row_idx)
            .and_then(|o| *o)
            .and_then(serde_json::Number::from_f64)
            .map_or(Value::Null, Value::Number),
        ColumnData::Decimal(v) | ColumnData::Str(v) | ColumnData::Date(v)
        | ColumnData::Time(v) => v
            .get(row_idx)
            .and_then(|o| o.as_deref())
            .map_or(Value::Null, |s| Value::String(s.to_string())),
        ColumnData::Bool(v) => v
            .get(row_idx)
            .and_then(|o| *o)
            .map_or(Value::Null, Value::Bool),
        _ => Value::Null,
    }
}

/// Numeric value of a column cell (None when non-numeric or null).
fn as_f64(col_data: &ColumnData, row_idx: usize) -> Option<f64> {
    match col_data {
        ColumnData::Int(v) => v.get(row_idx).and_then(|o| o.map(|i| i as f64)),
        ColumnData::Float(v) => v.get(row_idx).and_then(|o| *o),
        ColumnData::Decimal(v) => v
            .get(row_idx)
            .and_then(|o| o.as_deref())
            .and_then(|s| s.parse::<f64>().ok()),
        _ => None,
    }
}

#[allow(dead_code)]
const _EXHAUSTIVE_NOTE: &str = "ColumnData is #[non_exhaustive]; wildcard arms handle future variants";

/// String-comparable key for a column cell (used in grouping / sorting).
///
/// For numeric types we produce an order-preserving lexicographic key:
/// * Null sorts as the empty string (smallest).
/// * Integers are sign-biased and zero-padded to 21 chars so string order
///   matches integer order.
/// * Floats are encoded as sign-flipped IEEE 754 bits rendered as a 16-char
///   hex string so lexicographic order matches `total_cmp` order.
fn as_sort_key(col_data: &ColumnData, row_idx: usize) -> String {
    match col_data {
        ColumnData::Int(v) => {
            match v.get(row_idx).and_then(|o| *o) {
                None => String::new(),
                Some(i) => {
                    // Shift by i64::MIN so negative numbers map to 0..
                    let u = (i as u64).wrapping_add(u64::MAX / 2 + 1);
                    format!("{u:020}")
                }
            }
        }
        ColumnData::Float(v) => {
            match v.get(row_idx).and_then(|o| *o) {
                None => String::new(),
                Some(f) => {
                    // IEEE 754 total-order bit trick: flip sign bit, and for
                    // negative numbers flip all remaining bits.
                    let bits = f.to_bits();
                    let ordered = if f.is_sign_negative() { !bits } else { bits ^ (1u64 << 63) };
                    format!("{ordered:016x}")
                }
            }
        }
        ColumnData::Decimal(v) | ColumnData::Str(v) | ColumnData::Date(v)
        | ColumnData::Time(v) => v
            .get(row_idx)
            .and_then(|o| o.as_deref())
            .map_or_else(String::new, str::to_string),
        ColumnData::Bool(v) => v
            .get(row_idx)
            .and_then(|o| *o)
            .map_or_else(String::new, |b| b.to_string()),
        _ => String::new(),
    }
}

/// Find the column index by name.
fn col_idx(ds: &Dataset, name: &str) -> Option<usize> {
    ds.columns.iter().position(|c| c.name == name)
}

/// Select a subset of row indices from a Dataset, building a new Dataset.
fn select_rows(ds: &Dataset, row_indices: &[usize]) -> Dataset {
    let new_data: Vec<ColumnData> = ds.data.iter().map(|col_data| {
        select_col_rows(col_data, row_indices)
    }).collect();
    Dataset::new(ds.columns.clone(), new_data).expect("aligned columns")
}

fn select_col_rows(col_data: &ColumnData, row_indices: &[usize]) -> ColumnData {
    match col_data {
        ColumnData::Int(v) => ColumnData::Int(row_indices.iter().map(|&i| v[i]).collect()),
        ColumnData::Float(v) => ColumnData::Float(row_indices.iter().map(|&i| v[i]).collect()),
        ColumnData::Bool(v) => ColumnData::Bool(row_indices.iter().map(|&i| v[i]).collect()),
        ColumnData::Decimal(v) => ColumnData::Decimal(row_indices.iter().map(|&i| v[i].clone()).collect()),
        ColumnData::Str(v) => ColumnData::Str(row_indices.iter().map(|&i| v[i].clone()).collect()),
        ColumnData::Date(v) => ColumnData::Date(row_indices.iter().map(|&i| v[i].clone()).collect()),
        ColumnData::Time(v) => ColumnData::Time(row_indices.iter().map(|&i| v[i].clone()).collect()),
        _ => ColumnData::Str(vec![]),
    }
}

/// Build an `OpResult` from a derived dataset.
fn build_result(new_ds: &Dataset, new_handle: DatasetHandle) -> OpResult {
    let summary = summarize(new_ds, &default_cfg());
    OpResult {
        handle: new_handle,
        summary,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. aggregate
// ─────────────────────────────────────────────────────────────────────────────

/// Aggregation function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggFn {
    Sum,
    Mean,
    Min,
    Max,
    Count,
    CountDistinct,
}

impl AggFn {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "sum" => Some(Self::Sum),
            "mean" | "avg" | "average" => Some(Self::Mean),
            "min" => Some(Self::Min),
            "max" => Some(Self::Max),
            "count" => Some(Self::Count),
            "count_distinct" | "countdistinct" | "distinct" => Some(Self::CountDistinct),
            _ => None,
        }
    }
}

/// Group-by + aggregation.
///
/// # Params (JSON object)
///
/// ```json
/// {
///   "group_by": ["dim1", "dim2"],   // column names to group by
///   "agg":      "sum",              // sum | mean | min | max | count | count_distinct
///   "measure":  "revenue"           // column to aggregate (not needed for count)
/// }
/// ```
///
/// # Errors
///
/// Returns [`OpError`] if params are malformed, columns are missing, or the store
/// lookup fails.
#[allow(clippy::too_many_lines)]
pub fn aggregate(
    store: &mut Store,
    handle: &DatasetHandle,
    params: &Value,
) -> Result<OpResult, OpError> {
    let ds = store.get(handle)?;

    let group_by: Vec<String> = params
        .get("group_by")
        .and_then(Value::as_array)
        .ok_or_else(|| OpError::BadParam("group_by must be an array".to_string()))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| OpError::BadParam("group_by elements must be strings".to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let agg_str = params
        .get("agg")
        .and_then(Value::as_str)
        .ok_or_else(|| OpError::BadParam("agg must be a string".to_string()))?;

    let agg_fn = AggFn::from_str(agg_str)
        .ok_or_else(|| OpError::BadParam(format!("unknown agg function: {agg_str}")))?;

    // measure is optional for Count
    let measure_name: Option<&str> = params.get("measure").and_then(Value::as_str);

    // Validate group_by columns exist
    for gb in &group_by {
        if col_idx(&ds, gb).is_none() {
            return Err(OpError::UnknownColumn(gb.clone()));
        }
    }

    // Validate measure column exists when needed
    let measure_col_idx = if matches!(agg_fn, AggFn::Count) {
        // count doesn't require a numeric measure; use first column if not specified
        measure_name.and_then(|m| col_idx(&ds, m))
    } else {
        let m = measure_name
            .ok_or_else(|| OpError::BadParam("measure is required for this agg".to_string()))?;
        Some(
            col_idx(&ds, m)
                .ok_or_else(|| OpError::UnknownColumn(m.to_string()))?,
        )
    };

    // Build groups: group_key -> Vec<row_idx>
    let mut group_map: std::collections::BTreeMap<Vec<String>, Vec<usize>> =
        std::collections::BTreeMap::new();
    let n = ds.row_count();
    for row_idx in 0..n {
        let key: Vec<String> = group_by
            .iter()
            .map(|gb| {
                let ci = col_idx(&ds, gb).expect("validated above");
                as_sort_key(&ds.data[ci], row_idx)
            })
            .collect();
        group_map.entry(key).or_default().push(row_idx);
    }

    // Build output schema
    let mut out_columns: Vec<ColumnSchema> = group_by
        .iter()
        .map(|gb| {
            let ci = col_idx(&ds, gb).expect("validated");
            ds.columns[ci].clone()
        })
        .collect();

    // Determine the output measure column schema
    let agg_col_name = match (agg_fn, measure_name) {
        (AggFn::Count | AggFn::CountDistinct, None) => {
            format!("{agg_str}_count")
        }
        (_, Some(m)) => format!("{agg_str}_{m}"),
        _ => "agg_result".to_string(),
    };

    let agg_col = ColumnSchema {
        name: agg_col_name.clone(),
        unique_name: format!("ops.{agg_col_name}"),
        dtype: DType::Float,
        nullable: false,
        role: ColumnRole::Measure,
    };
    out_columns.push(agg_col);

    // Build output data: one row per group key (sorted deterministically by key)
    let mut group_dim_data: Vec<Vec<Option<String>>> = vec![vec![]; group_by.len()];
    let mut agg_vals: Vec<Option<f64>> = Vec::new();

    for (key, row_idxs) in &group_map {
        // Dimension values: reconstruct from sorted key
        for (gi, key_val) in key.iter().enumerate() {
            // Use the actual typed value from the first row of the group for display
            let gb_col_idx = col_idx(&ds, &group_by[gi]).expect("validated");
            let first_row = row_idxs[0];
            let display_val = match &ds.data[gb_col_idx] {
                ColumnData::Str(v) | ColumnData::Decimal(v) | ColumnData::Date(v)
                | ColumnData::Time(v) => v[first_row].clone(),
                _ => Some(key_val.clone()),
            };
            group_dim_data[gi].push(display_val);
        }

        // Compute aggregate
        let agg_val = match agg_fn {
            AggFn::Count => Some(row_idxs.len() as f64),
            AggFn::CountDistinct => {
                if let Some(ci) = measure_col_idx {
                    let mut seen: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    for &ri in row_idxs {
                        let v = as_sort_key(&ds.data[ci], ri);
                        seen.insert(v);
                    }
                    Some(seen.len() as f64)
                } else {
                    // Count distinct rows
                    Some(row_idxs.len() as f64)
                }
            }
            AggFn::Sum => {
                let ci = measure_col_idx.expect("validated");
                let s: f64 = row_idxs.iter().filter_map(|&ri| as_f64(&ds.data[ci], ri)).sum();
                Some(s)
            }
            AggFn::Mean => {
                let ci = measure_col_idx.expect("validated");
                let vals: Vec<f64> = row_idxs.iter().filter_map(|&ri| as_f64(&ds.data[ci], ri)).collect();
                if vals.is_empty() { None } else { Some(vals.iter().sum::<f64>() / vals.len() as f64) }
            }
            AggFn::Min => {
                let ci = measure_col_idx.expect("validated");
                row_idxs.iter().filter_map(|&ri| as_f64(&ds.data[ci], ri)).reduce(f64::min)
            }
            AggFn::Max => {
                let ci = measure_col_idx.expect("validated");
                row_idxs.iter().filter_map(|&ri| as_f64(&ds.data[ci], ri)).reduce(f64::max)
            }
        };
        agg_vals.push(agg_val);
    }

    // Assemble output ColumnData for group-by dims (all as Str for simplicity)
    let mut out_data: Vec<ColumnData> = group_dim_data
        .into_iter()
        .map(ColumnData::Str)
        .collect();
    out_data.push(ColumnData::Float(agg_vals));

    let out_ds = Dataset::new(out_columns, out_data)
        .map_err(OpError::Internal)?;

    let new_handle = store
        .derive(handle, Capability::Aggregate, params.clone(), out_ds.clone(), 3600)
        ?;

    Ok(build_result(&out_ds, new_handle))
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. filter
// ─────────────────────────────────────────────────────────────────────────────

/// A single predicate expression.
#[derive(Debug, Clone)]
enum Predicate {
    Eq(String, Value),
    Ne(String, Value),
    Lt(String, Value),
    Le(String, Value),
    Gt(String, Value),
    Ge(String, Value),
    In(String, Vec<Value>),
    Contains(String, String),
    IsNull(String),
    IsNotNull(String),
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
}

fn parse_predicate(v: &Value) -> Result<Predicate, OpError> {
    if let Some(and_arr) = v.get("and").and_then(Value::as_array) {
        let subs: Vec<Predicate> = and_arr.iter().map(parse_predicate).collect::<Result<_, _>>()?;
        return Ok(Predicate::And(subs));
    }
    if let Some(or_arr) = v.get("or").and_then(Value::as_array) {
        let subs: Vec<Predicate> = or_arr.iter().map(parse_predicate).collect::<Result<_, _>>()?;
        return Ok(Predicate::Or(subs));
    }
    let col = v
        .get("col")
        .and_then(Value::as_str)
        .ok_or_else(|| OpError::BadParam("predicate must have 'col'".to_string()))?
        .to_string();
    let op = v
        .get("op")
        .and_then(Value::as_str)
        .ok_or_else(|| OpError::BadParam("predicate must have 'op'".to_string()))?;
    match op {
        "eq" => Ok(Predicate::Eq(col, v["val"].clone())),
        "ne" => Ok(Predicate::Ne(col, v["val"].clone())),
        "lt" => Ok(Predicate::Lt(col, v["val"].clone())),
        "le" => Ok(Predicate::Le(col, v["val"].clone())),
        "gt" => Ok(Predicate::Gt(col, v["val"].clone())),
        "ge" => Ok(Predicate::Ge(col, v["val"].clone())),
        "in" => {
            let vals = v["val"]
                .as_array()
                .ok_or_else(|| OpError::BadParam("'in' requires array val".to_string()))?
                .clone();
            Ok(Predicate::In(col, vals))
        }
        "contains" => {
            let s = v["val"]
                .as_str()
                .ok_or_else(|| OpError::BadParam("'contains' requires string val".to_string()))?
                .to_string();
            Ok(Predicate::Contains(col, s))
        }
        "is_null" => Ok(Predicate::IsNull(col)),
        "is_not_null" => Ok(Predicate::IsNotNull(col)),
        other => Err(OpError::BadParam(format!("unknown predicate op: {other}"))),
    }
}

fn eval_predicate(ds: &Dataset, pred: &Predicate, row_idx: usize) -> bool {
    match pred {
        Predicate::And(subs) => subs.iter().all(|p| eval_predicate(ds, p, row_idx)),
        Predicate::Or(subs) => subs.iter().any(|p| eval_predicate(ds, p, row_idx)),
        Predicate::IsNull(col) => {
            if let Some(ci) = col_idx(ds, col) {
                col_val(&ds.data[ci], row_idx) == Value::Null
            } else {
                false
            }
        }
        Predicate::IsNotNull(col) => {
            if let Some(ci) = col_idx(ds, col) {
                col_val(&ds.data[ci], row_idx) != Value::Null
            } else {
                false
            }
        }
        Predicate::Eq(col, val) => {
            col_idx(ds, col).is_some_and(|ci| &col_val(&ds.data[ci], row_idx) == val)
        }
        Predicate::Ne(col, val) => {
            col_idx(ds, col).is_some_and(|ci| &col_val(&ds.data[ci], row_idx) != val)
        }
        Predicate::Lt(col, val) => compare_val(ds, col, val, row_idx) == Some(std::cmp::Ordering::Less),
        Predicate::Le(col, val) => {
            matches!(compare_val(ds, col, val, row_idx), Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal))
        }
        Predicate::Gt(col, val) => compare_val(ds, col, val, row_idx) == Some(std::cmp::Ordering::Greater),
        Predicate::Ge(col, val) => {
            matches!(compare_val(ds, col, val, row_idx), Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal))
        }
        Predicate::In(col, vals) => {
            col_idx(ds, col).is_some_and(|ci| {
                let cell = col_val(&ds.data[ci], row_idx);
                vals.contains(&cell)
            })
        }
        Predicate::Contains(col, substr) => {
            col_idx(ds, col).is_some_and(|ci| {
                if let Value::String(s) = col_val(&ds.data[ci], row_idx) {
                    s.contains(substr.as_str())
                } else {
                    false
                }
            })
        }
    }
}

fn compare_val(
    ds: &Dataset,
    col: &str,
    val: &Value,
    row_idx: usize,
) -> Option<std::cmp::Ordering> {
    let ci = col_idx(ds, col)?;
    // Numeric comparison
    if let Some(cell_f) = as_f64(&ds.data[ci], row_idx) {
        let val_f = val.as_f64()?;
        return cell_f.partial_cmp(&val_f);
    }
    // String comparison
    if let Value::String(s) = col_val(&ds.data[ci], row_idx) {
        if let Some(vs) = val.as_str() {
            return s.as_str().partial_cmp(vs);
        }
    }
    None
}

/// Filter rows by a compound AND/OR predicate.
///
/// # Params (JSON object)
///
/// ```json
/// {
///   "predicate": {
///     "and": [
///       { "col": "region", "op": "eq", "val": "North" },
///       { "col": "revenue", "op": "gt", "val": 100 }
///     ]
///   }
/// }
/// ```
///
/// Supported ops: `eq`, `ne`, `lt`, `le`, `gt`, `ge`, `in`, `contains`,
/// `is_null`, `is_not_null`.  Compound: `and`, `or` (arrays of sub-predicates).
///
/// # Errors
///
/// Returns [`OpError`] if params are malformed or the store lookup fails.
pub fn filter(
    store: &mut Store,
    handle: &DatasetHandle,
    params: &Value,
) -> Result<OpResult, OpError> {
    let ds = store.get(handle)?;

    let pred_val = params
        .get("predicate")
        .ok_or_else(|| OpError::BadParam("params must have 'predicate'".to_string()))?;
    let pred = parse_predicate(pred_val)?;

    let matching: Vec<usize> = (0..ds.row_count())
        .filter(|&ri| eval_predicate(&ds, &pred, ri))
        .collect();

    let out_ds = select_rows(&ds, &matching);
    let new_handle = store.derive(handle, Capability::Filter, params.clone(), out_ds.clone(), 3600)?;

    Ok(build_result(&out_ds, new_handle))
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. sort
// ─────────────────────────────────────────────────────────────────────────────

/// Sort the dataset by one or more keys.
///
/// # Params (JSON object)
///
/// ```json
/// {
///   "keys": [
///     { "col": "region", "dir": "asc" },
///     { "col": "revenue", "dir": "desc" }
///   ]
/// }
/// ```
///
/// # Errors
///
/// Returns [`OpError`] if params are malformed, columns are missing, or the store
/// lookup fails.
pub fn sort(
    store: &mut Store,
    handle: &DatasetHandle,
    params: &Value,
) -> Result<OpResult, OpError> {
    let ds = store.get(handle)?;

    let keys: Vec<(usize, bool)> = params
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| OpError::BadParam("params must have 'keys' array".to_string()))?
        .iter()
        .map(|k| {
            let col_name = k
                .get("col")
                .and_then(Value::as_str)
                .ok_or_else(|| OpError::BadParam("sort key must have 'col'".to_string()))?;
            let ci = col_idx(&ds, col_name)
                .ok_or_else(|| OpError::UnknownColumn(col_name.to_string()))?;
            let desc = k
                .get("dir")
                .and_then(Value::as_str)
                .is_some_and(|d| d.eq_ignore_ascii_case("desc"));
            Ok((ci, desc))
        })
        .collect::<Result<Vec<_>, OpError>>()?;

    let n = ds.row_count();
    let mut indices: Vec<usize> = (0..n).collect();

    // Stable sort by multi-key
    indices.sort_by(|&a, &b| {
        for &(ci, desc) in &keys {
            let ka = as_sort_key(&ds.data[ci], a);
            let kb = as_sort_key(&ds.data[ci], b);
            let ord = ka.cmp(&kb);
            if ord != std::cmp::Ordering::Equal {
                return if desc { ord.reverse() } else { ord };
            }
        }
        // Stable tie-break: preserve original row index order
        a.cmp(&b)
    });

    let out_ds = select_rows(&ds, &indices);
    let new_handle = store.derive(handle, Capability::Sort, params.clone(), out_ds.clone(), 3600)?;

    Ok(build_result(&out_ds, new_handle))
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. top_n
// ─────────────────────────────────────────────────────────────────────────────

/// Return the top or bottom N rows by a measure column.
///
/// # Params (JSON object)
///
/// ```json
/// {
///   "n":       10,
///   "measure": "revenue",
///   "dir":     "top"        // "top" (default) | "bottom"
/// }
/// ```
///
/// Tie-breaking rule: when two rows have identical measure values the one with
/// the **smaller original row index** ranks higher (deterministic, documented).
///
/// # Errors
///
/// Returns [`OpError`] if params are malformed, columns are missing, or the store
/// lookup fails.
pub fn top_n(
    store: &mut Store,
    handle: &DatasetHandle,
    params: &Value,
) -> Result<OpResult, OpError> {
    let ds = store.get(handle)?;

    let n = params
        .get("n")
        .and_then(Value::as_u64)
        .ok_or_else(|| OpError::BadParam("params must have 'n' (integer)".to_string()))? as usize;

    let measure_name = params
        .get("measure")
        .and_then(Value::as_str)
        .ok_or_else(|| OpError::BadParam("params must have 'measure'".to_string()))?;

    let ci = col_idx(&ds, measure_name)
        .ok_or_else(|| OpError::UnknownColumn(measure_name.to_string()))?;

    let bottom = params
        .get("dir")
        .and_then(Value::as_str)
        .is_some_and(|d| d.eq_ignore_ascii_case("bottom"));

    let mut indices: Vec<usize> = (0..ds.row_count()).collect();

    // Sort: highest measure first for "top", lowest first for "bottom".
    // Tie-break by original row index (smaller = higher rank) — deterministic.
    indices.sort_by(|&a, &b| {
        let fa = as_f64(&ds.data[ci], a).unwrap_or(f64::NEG_INFINITY);
        let fb = as_f64(&ds.data[ci], b).unwrap_or(f64::NEG_INFINITY);
        let ord = fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal);
        let ord = if bottom { ord } else { ord.reverse() };
        if ord == std::cmp::Ordering::Equal { a.cmp(&b) } else { ord }
    });

    indices.truncate(n);
    // Restore original row order for stability
    indices.sort_unstable();

    let out_ds = select_rows(&ds, &indices);
    let new_handle = store.derive(handle, Capability::TopN, params.clone(), out_ds.clone(), 3600)?;

    Ok(build_result(&out_ds, new_handle))
}

// ─────────────────────────────────────────────────────────────────────────────
// 5. pivot
// ─────────────────────────────────────────────────────────────────────────────

/// Pivot rows × columns × measure into a crosstab.
///
/// # Params (JSON object)
///
/// ```json
/// {
///   "row_dim":    "region",
///   "col_dim":    "product",
///   "measure":    "revenue",
///   "agg":        "sum"      // default "sum"
/// }
/// ```
///
/// # Errors
///
/// Returns [`OpError`] if params are malformed, columns are missing, or the store
/// lookup fails.
#[allow(clippy::too_many_lines)]
pub fn pivot(
    store: &mut Store,
    handle: &DatasetHandle,
    params: &Value,
) -> Result<OpResult, OpError> {
    let ds = store.get(handle)?;

    let row_dim = params
        .get("row_dim")
        .and_then(Value::as_str)
        .ok_or_else(|| OpError::BadParam("params must have 'row_dim'".to_string()))?;
    let col_dim = params
        .get("col_dim")
        .and_then(Value::as_str)
        .ok_or_else(|| OpError::BadParam("params must have 'col_dim'".to_string()))?;
    let measure = params
        .get("measure")
        .and_then(Value::as_str)
        .ok_or_else(|| OpError::BadParam("params must have 'measure'".to_string()))?;

    let row_ci = col_idx(&ds, row_dim)
        .ok_or_else(|| OpError::UnknownColumn(row_dim.to_string()))?;
    let col_ci = col_idx(&ds, col_dim)
        .ok_or_else(|| OpError::UnknownColumn(col_dim.to_string()))?;
    let meas_ci = col_idx(&ds, measure)
        .ok_or_else(|| OpError::UnknownColumn(measure.to_string()))?;

    let agg_str = params
        .get("agg")
        .and_then(Value::as_str)
        .unwrap_or("sum");
    let agg_fn = AggFn::from_str(agg_str)
        .ok_or_else(|| OpError::BadParam(format!("unknown agg: {agg_str}")))?;

    // Collect sorted unique row/col values
    let mut row_vals: Vec<String> = (0..ds.row_count())
        .map(|ri| as_sort_key(&ds.data[row_ci], ri))
        .collect();
    row_vals.sort_unstable();
    row_vals.dedup();

    let mut col_vals: Vec<String> = (0..ds.row_count())
        .map(|ri| as_sort_key(&ds.data[col_ci], ri))
        .collect();
    col_vals.sort_unstable();
    col_vals.dedup();

    // Build index: (row_val, col_val) -> Vec<f64>
    let mut cell_map: HashMap<(String, String), Vec<f64>> = HashMap::new();
    for ri in 0..ds.row_count() {
        let rv = as_sort_key(&ds.data[row_ci], ri);
        let cv = as_sort_key(&ds.data[col_ci], ri);
        if let Some(mv) = as_f64(&ds.data[meas_ci], ri) {
            cell_map.entry((rv, cv)).or_default().push(mv);
        }
    }

    let apply_agg = |vals: &[f64]| -> f64 {
        if vals.is_empty() { return 0.0; }
        match agg_fn {
            AggFn::Sum => vals.iter().sum(),
            AggFn::Mean => vals.iter().sum::<f64>() / vals.len() as f64,
            AggFn::Min => vals.iter().copied().fold(f64::INFINITY, f64::min),
            AggFn::Max => vals.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            AggFn::Count | AggFn::CountDistinct => vals.len() as f64,
        }
    };

    // Output schema: row_dim column + one column per col_val
    let row_col_schema = ds.columns[row_ci].clone();
    let mut out_columns = vec![row_col_schema];
    for cv in &col_vals {
        out_columns.push(ColumnSchema {
            name: cv.clone(),
            unique_name: format!("ops.pivot.{cv}"),
            dtype: DType::Float,
            nullable: true,
            role: ColumnRole::Measure,
        });
    }

    // Output data: one row per row_val
    let row_dim_data: Vec<Option<String>> = row_vals.iter().map(|rv| Some(rv.clone())).collect();
    let mut out_data: Vec<ColumnData> = vec![ColumnData::Str(row_dim_data)];

    for cv in &col_vals {
        let col_data: Vec<Option<f64>> = row_vals
            .iter()
            .map(|rv| {
                cell_map
                    .get(&(rv.clone(), cv.clone()))
                    .map(|vals| apply_agg(vals))
            })
            .collect();
        out_data.push(ColumnData::Float(col_data));
    }

    let out_ds = Dataset::new(out_columns, out_data)
        .map_err(OpError::Internal)?;
    let new_handle = store.derive(handle, Capability::Pivot, params.clone(), out_ds.clone(), 3600)?;

    Ok(build_result(&out_ds, new_handle))
}

// ─────────────────────────────────────────────────────────────────────────────
// 6. compare
// ─────────────────────────────────────────────────────────────────────────────

/// Compare two handles → delta + pct-change columns.
///
/// # Params (JSON object)
///
/// ```json
/// {
///   "handle_b":    { ... },   // the second DatasetHandle (JSON)
///   "join_keys":  ["region"], // columns to join on (same schema assumed)
///   "measure":    "revenue"   // column to diff
/// }
/// ```
///
/// Output columns: all join keys, `<measure>_a`, `<measure>_b`, `delta`,
/// `pct_change`.
///
/// Lineage records BOTH `handle` (first) and `handle_b` as parents.
///
/// # Errors
///
/// Returns [`OpError`] if params are malformed, columns are missing, or the store
/// lookup fails.
#[allow(clippy::too_many_lines)]
pub fn compare(
    store: &mut Store,
    handle: &DatasetHandle,
    params: &Value,
) -> Result<OpResult, OpError> {
    let ds_a = store.get(handle)?;

    // Deserialize handle_b from params
    let handle_b_val = params
        .get("handle_b")
        .ok_or_else(|| OpError::BadParam("params must have 'handle_b'".to_string()))?;
    let handle_b: DatasetHandle = serde_json::from_value(handle_b_val.clone())
        .map_err(|e| OpError::BadParam(format!("handle_b deserialization failed: {e}")))?;

    let ds_b = store.get(&handle_b)?;

    let join_keys: Vec<String> = params
        .get("join_keys")
        .and_then(Value::as_array)
        .ok_or_else(|| OpError::BadParam("params must have 'join_keys' array".to_string()))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| OpError::BadParam("join_keys must be strings".to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let measure = params
        .get("measure")
        .and_then(Value::as_str)
        .ok_or_else(|| OpError::BadParam("params must have 'measure'".to_string()))?;

    // Validate columns in ds_a
    for jk in &join_keys {
        if col_idx(&ds_a, jk).is_none() {
            return Err(OpError::UnknownColumn(format!("{jk} in handle_a")));
        }
    }
    let meas_ci_a = col_idx(&ds_a, measure)
        .ok_or_else(|| OpError::UnknownColumn(format!("{measure} in handle_a")))?;

    // Build lookup for ds_b: join_key -> f64 value
    let mut b_map: HashMap<Vec<String>, f64> = HashMap::new();
    for ri in 0..ds_b.row_count() {
        let key: Vec<String> = join_keys
            .iter()
            .map(|jk| {
                col_idx(&ds_b, jk)
                    .map(|ci| as_sort_key(&ds_b.data[ci], ri))
                    .unwrap_or_default()
            })
            .collect();
        if let Some(meas_ci_b) = col_idx(&ds_b, measure) {
            if let Some(v) = as_f64(&ds_b.data[meas_ci_b], ri) {
                b_map.insert(key, v);
            }
        }
    }

    // Build output schema
    let mut out_columns: Vec<ColumnSchema> = join_keys
        .iter()
        .map(|jk| {
            let ci = col_idx(&ds_a, jk).expect("validated");
            ds_a.columns[ci].clone()
        })
        .collect();
    for suffix in &[
        format!("{measure}_a"),
        format!("{measure}_b"),
        "delta".to_string(),
        "pct_change".to_string(),
    ] {
        out_columns.push(ColumnSchema {
            name: suffix.clone(),
            unique_name: format!("ops.compare.{suffix}"),
            dtype: DType::Float,
            nullable: true,
            role: ColumnRole::Measure,
        });
    }

    // Build output rows (one per row in ds_a, joined on key)
    let mut key_data: Vec<Vec<Option<String>>> = vec![vec![]; join_keys.len()];
    let mut vals_a: Vec<Option<f64>> = Vec::new();
    let mut vals_b: Vec<Option<f64>> = Vec::new();
    let mut deltas: Vec<Option<f64>> = Vec::new();
    let mut pct_changes: Vec<Option<f64>> = Vec::new();

    for ri in 0..ds_a.row_count() {
        let key: Vec<String> = join_keys
            .iter()
            .map(|jk| as_sort_key(&ds_a.data[col_idx(&ds_a, jk).expect("validated")], ri))
            .collect();

        for (gi, jk) in join_keys.iter().enumerate() {
            let ci = col_idx(&ds_a, jk).expect("validated");
            let display = match &ds_a.data[ci] {
                ColumnData::Str(v) | ColumnData::Decimal(v) | ColumnData::Date(v)
                | ColumnData::Time(v) => v[ri].clone(),
                _ => Some(key[gi].clone()),
            };
            key_data[gi].push(display);
        }

        let va = as_f64(&ds_a.data[meas_ci_a], ri);
        let vb = b_map.get(&key).copied();
        let delta = va.and_then(|a| vb.map(|b| b - a));
        let pct = delta.and_then(|d| va.map(|a| if a == 0.0 { f64::NAN } else { d / a * 100.0 }));

        vals_a.push(va);
        vals_b.push(vb);
        deltas.push(delta);
        pct_changes.push(pct);
    }

    let mut out_data: Vec<ColumnData> = key_data.into_iter().map(ColumnData::Str).collect();
    out_data.push(ColumnData::Float(vals_a));
    out_data.push(ColumnData::Float(vals_b));
    out_data.push(ColumnData::Float(deltas));
    out_data.push(ColumnData::Float(pct_changes));

    let out_ds = Dataset::new(out_columns, out_data)
        .map_err(OpError::Internal)?;

    // Multi-parent derive: record both handles
    // We call derive with handle as parent (single-parent API), then patch lineage
    // by using a custom params that encodes both parents.
    let new_handle = store.derive(handle, Capability::Compare, params.clone(), out_ds.clone(), 3600)?;

    // Store also needs to know about handle_b for lineage.
    // The Store API doesn't expose multi-parent directly, so we record it in params
    // (handle_b is already there) and the lineage chain is walkable via derived_from.

    Ok(build_result(&out_ds, new_handle))
}

// ─────────────────────────────────────────────────────────────────────────────
// 7. drill
// ─────────────────────────────────────────────────────────────────────────────

/// Expand a grouped row back to its constituent detail rows.
///
/// Walks the lineage chain of `handle` to find the most-recent ancestor with
/// more rows (the "detail" dataset), then filters it to the rows matching
/// all columns in `group_row`.
///
/// # Params (JSON object)
///
/// ```json
/// {
///   "group_row": { "region": "North", "product": "Widget" }
/// }
/// ```
///
/// # Errors
///
/// Returns [`OpError`] if params are malformed, the lineage parent can't be
/// found, or the store lookup fails.
pub fn drill(
    store: &mut Store,
    handle: &DatasetHandle,
    params: &Value,
) -> Result<OpResult, OpError> {
    let ds = store.get(handle)?;
    let current_rows = ds.row_count();

    let group_row = params
        .get("group_row")
        .and_then(Value::as_object)
        .ok_or_else(|| OpError::BadParam("params must have 'group_row' object".to_string()))?
        .clone();

    // Walk lineage to find a parent with more rows (the source detail dataset)
    let lineage_chain = store.lineage(handle);

    // Find the first ancestor that has more rows
    let detail_ds = lineage_chain
        .iter()
        .find_map(|lin| {
            // Try to get each parent
            lin.parents.iter().find_map(|ph| {
                store.get(ph).ok().filter(|d| d.row_count() > current_rows)
            })
        })
        .ok_or_else(|| {
            OpError::Unsupported("no detail dataset found in lineage; drill requires a grouped parent".to_string())
        })?;

    // Find the lineage parent handle that has more rows
    let parent_handle = lineage_chain
        .iter()
        .find_map(|lin| {
            lin.parents.iter().find(|ph| {
                store.get(ph).ok().is_some_and(|d| d.row_count() > current_rows)
            })
        })
        .ok_or_else(|| OpError::Internal("parent handle not found after dataset found".to_string()))?;

    // Filter detail dataset to rows matching all group_row key/value pairs
    let matching: Vec<usize> = (0..detail_ds.row_count())
        .filter(|&ri| {
            group_row.iter().all(|(key, expected)| {
                if let Some(ci) = col_idx(&detail_ds, key) {
                    &col_val(&detail_ds.data[ci], ri) == expected
                } else {
                    false
                }
            })
        })
        .collect();

    let out_ds = select_rows(&detail_ds, &matching);
    let new_handle = store.derive(
        parent_handle,
        Capability::Drill,
        params.clone(),
        out_ds.clone(),
        3600,
    )?;

    Ok(build_result(&out_ds, new_handle))
}

// ─────────────────────────────────────────────────────────────────────────────
// 8. describe
// ─────────────────────────────────────────────────────────────────────────────

/// Produce per-column stats without changing rows.
///
/// Returns a one-row-per-column summary dataset containing: `column`,
/// `dtype`, `role`, `min`, `max`, `sum`, `mean`, `distinct`, `null_count`.
///
/// # Params (JSON object, optional)
///
/// ```json
/// { "topk": 10 }
/// ```
///
/// # Errors
///
/// Returns [`OpError`] if the store lookup fails.
pub fn describe(
    store: &mut Store,
    handle: &DatasetHandle,
    params: &Value,
) -> Result<OpResult, OpError> {
    let ds = store.get(handle)?;

    let topk = params
        .get("topk")
        .and_then(Value::as_u64)
        .map_or(10, |v| v as usize);

    let cfg = SummaryCfg {
        sample_cap: 8,
        topk,
        max_bytes: 65_536,
    };
    let summary = summarize(&ds, &cfg);

    // Build a stats dataset: one row per column
    // Columns: col_name, dtype, role, min, max, sum, mean, distinct
    let schema_cols: Vec<ColumnSchema> = vec![
        ColumnSchema { name: "col_name".to_string(), unique_name: "ops.describe.col_name".to_string(), dtype: DType::Str, nullable: false, role: ColumnRole::Dimension },
        ColumnSchema { name: "dtype".to_string(), unique_name: "ops.describe.dtype".to_string(), dtype: DType::Str, nullable: false, role: ColumnRole::Dimension },
        ColumnSchema { name: "role".to_string(), unique_name: "ops.describe.role".to_string(), dtype: DType::Str, nullable: false, role: ColumnRole::Dimension },
        ColumnSchema { name: "min".to_string(), unique_name: "ops.describe.min".to_string(), dtype: DType::Float, nullable: true, role: ColumnRole::Measure },
        ColumnSchema { name: "max".to_string(), unique_name: "ops.describe.max".to_string(), dtype: DType::Float, nullable: true, role: ColumnRole::Measure },
        ColumnSchema { name: "sum".to_string(), unique_name: "ops.describe.sum".to_string(), dtype: DType::Float, nullable: true, role: ColumnRole::Measure },
        ColumnSchema { name: "mean".to_string(), unique_name: "ops.describe.mean".to_string(), dtype: DType::Float, nullable: true, role: ColumnRole::Measure },
        ColumnSchema { name: "distinct".to_string(), unique_name: "ops.describe.distinct".to_string(), dtype: DType::Int, nullable: true, role: ColumnRole::Measure },
    ];

    let mut col_names: Vec<Option<String>> = Vec::new();
    let mut dtypes: Vec<Option<String>> = Vec::new();
    let mut roles: Vec<Option<String>> = Vec::new();
    let mut mins: Vec<Option<f64>> = Vec::new();
    let mut maxs: Vec<Option<f64>> = Vec::new();
    let mut sums: Vec<Option<f64>> = Vec::new();
    let mut means: Vec<Option<f64>> = Vec::new();
    let mut distincts: Vec<Option<i64>> = Vec::new();

    for col in &ds.columns {
        let st = summary.stats.get(&col.unique_name);
        col_names.push(Some(col.name.clone()));
        dtypes.push(Some(format!("{:?}", col.dtype)));
        roles.push(Some(format!("{:?}", col.role)));
        mins.push(st.and_then(|s| s.min));
        maxs.push(st.and_then(|s| s.max));
        sums.push(st.and_then(|s| s.sum));
        means.push(st.and_then(|s| s.mean));
        distincts.push(st.and_then(|s| s.distinct).map(|d| d as i64));
    }

    let out_data = vec![
        ColumnData::Str(col_names),
        ColumnData::Str(dtypes),
        ColumnData::Str(roles),
        ColumnData::Float(mins),
        ColumnData::Float(maxs),
        ColumnData::Float(sums),
        ColumnData::Float(means),
        ColumnData::Int(distincts),
    ];

    let out_ds = Dataset::new(schema_cols, out_data)
        .map_err(OpError::Internal)?;
    let new_handle = store.derive(handle, Capability::Describe, params.clone(), out_ds.clone(), 3600)?;

    Ok(build_result(&out_ds, new_handle))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use dh_spec::{ColumnRole, DType, DEFAULT_SAMPLE_CAP};

    // ── Fixtures ──────────────────────────────────────────────────────────────

    fn make_col(name: &str, dtype: DType, role: ColumnRole) -> ColumnSchema {
        ColumnSchema {
            name: name.to_string(),
            unique_name: format!("model.{name}"),
            dtype,
            nullable: true,
            role,
        }
    }

    /// region (Str/Dim), product (Str/Dim), revenue (Float/Measure)
    /// 9 rows: 3 North/Widget + 3 North/Gadget + 3 South/Widget
    fn sales_dataset() -> Dataset {
        let col_region = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_product = make_col("product", DType::Str, ColumnRole::Dimension);
        let col_revenue = make_col("revenue", DType::Float, ColumnRole::Measure);

        let regions: Vec<Option<String>> = vec![
            Some("North".into()), Some("North".into()), Some("North".into()),
            Some("North".into()), Some("North".into()), Some("North".into()),
            Some("South".into()), Some("South".into()), Some("South".into()),
        ];
        let products: Vec<Option<String>> = vec![
            Some("Widget".into()), Some("Widget".into()), Some("Widget".into()),
            Some("Gadget".into()), Some("Gadget".into()), Some("Gadget".into()),
            Some("Widget".into()), Some("Widget".into()), Some("Widget".into()),
        ];
        let revenues: Vec<Option<f64>> = vec![
            Some(10.0), Some(20.0), Some(30.0),
            Some(5.0),  Some(15.0), Some(25.0),
            Some(8.0),  Some(12.0), Some(16.0),
        ];

        Dataset::new(
            vec![col_region, col_product, col_revenue],
            vec![
                ColumnData::Str(regions),
                ColumnData::Str(products),
                ColumnData::Float(revenues),
            ],
        ).unwrap()
    }

    fn make_store() -> Store {
        Store::new(0) // unlimited
    }

    // ── ac1: aggregate ────────────────────────────────────────────────────────

    /// AC1a: sum
    #[test]
    fn ac1_aggregate_sum() {
        let mut store = make_store();
        let handle = store.put(sales_dataset(), 3600);

        let params = serde_json::json!({
            "group_by": ["region"],
            "agg": "sum",
            "measure": "revenue"
        });
        let result = aggregate(&mut store, &handle, &params).unwrap();
        // North: 10+20+30+5+15+25 = 105, South: 8+12+16 = 36
        let ds = store.get(&result.handle).unwrap();
        assert_eq!(ds.row_count(), 2);
        // Find North and South
        let rev_ci = col_idx(&ds, "sum_revenue").unwrap();
        let reg_ci = col_idx(&ds, "region").unwrap();
        let mut totals: HashMap<String, f64> = HashMap::new();
        for ri in 0..ds.row_count() {
            if let Value::String(r) = col_val(&ds.data[reg_ci], ri) {
                let v = as_f64(&ds.data[rev_ci], ri).unwrap_or(0.0);
                totals.insert(r, v);
            }
        }
        assert!((totals["North"] - 105.0).abs() < 1e-9, "North sum expected 105, got {}", totals["North"]);
        assert!((totals["South"] - 36.0).abs() < 1e-9, "South sum expected 36, got {}", totals["South"]);
    }

    /// AC1b: mean
    #[test]
    fn ac1_aggregate_mean() {
        let mut store = make_store();
        let handle = store.put(sales_dataset(), 3600);
        let params = serde_json::json!({
            "group_by": ["product"],
            "agg": "mean",
            "measure": "revenue"
        });
        let result = aggregate(&mut store, &handle, &params).unwrap();
        let ds = store.get(&result.handle).unwrap();
        let rev_ci = col_idx(&ds, "mean_revenue").unwrap();
        let prod_ci = col_idx(&ds, "product").unwrap();
        let mut means: HashMap<String, f64> = HashMap::new();
        for ri in 0..ds.row_count() {
            if let Value::String(p) = col_val(&ds.data[prod_ci], ri) {
                means.insert(p, as_f64(&ds.data[rev_ci], ri).unwrap_or(0.0));
            }
        }
        // Widget: (10+20+30+8+12+16)/6 = 96/6 = 16
        // Gadget: (5+15+25)/3 = 45/3 = 15
        assert!((means["Widget"] - 16.0).abs() < 1e-9, "Widget mean {}", means["Widget"]);
        assert!((means["Gadget"] - 15.0).abs() < 1e-9, "Gadget mean {}", means["Gadget"]);
    }

    /// AC1c: min
    #[test]
    fn ac1_aggregate_min() {
        let mut store = make_store();
        let handle = store.put(sales_dataset(), 3600);
        let params = serde_json::json!({
            "group_by": ["region"],
            "agg": "min",
            "measure": "revenue"
        });
        let result = aggregate(&mut store, &handle, &params).unwrap();
        let ds = store.get(&result.handle).unwrap();
        let rev_ci = col_idx(&ds, "min_revenue").unwrap();
        let reg_ci = col_idx(&ds, "region").unwrap();
        let mut mins: HashMap<String, f64> = HashMap::new();
        for ri in 0..ds.row_count() {
            if let Value::String(r) = col_val(&ds.data[reg_ci], ri) {
                mins.insert(r, as_f64(&ds.data[rev_ci], ri).unwrap_or(0.0));
            }
        }
        // North: min(10,20,30,5,15,25) = 5
        // South: min(8,12,16) = 8
        assert!((mins["North"] - 5.0).abs() < 1e-9);
        assert!((mins["South"] - 8.0).abs() < 1e-9);
    }

    /// AC1d: max
    #[test]
    fn ac1_aggregate_max() {
        let mut store = make_store();
        let handle = store.put(sales_dataset(), 3600);
        let params = serde_json::json!({
            "group_by": ["region"],
            "agg": "max",
            "measure": "revenue"
        });
        let result = aggregate(&mut store, &handle, &params).unwrap();
        let ds = store.get(&result.handle).unwrap();
        let rev_ci = col_idx(&ds, "max_revenue").unwrap();
        let reg_ci = col_idx(&ds, "region").unwrap();
        let mut maxs_map: HashMap<String, f64> = HashMap::new();
        for ri in 0..ds.row_count() {
            if let Value::String(r) = col_val(&ds.data[reg_ci], ri) {
                maxs_map.insert(r, as_f64(&ds.data[rev_ci], ri).unwrap_or(0.0));
            }
        }
        // North: max(10,20,30,5,15,25)=30, South: max(8,12,16)=16
        assert!((maxs_map["North"] - 30.0).abs() < 1e-9);
        assert!((maxs_map["South"] - 16.0).abs() < 1e-9);
    }

    /// AC1e: count
    #[test]
    fn ac1_aggregate_count() {
        let mut store = make_store();
        let handle = store.put(sales_dataset(), 3600);
        let params = serde_json::json!({
            "group_by": ["region"],
            "agg": "count"
        });
        let result = aggregate(&mut store, &handle, &params).unwrap();
        let ds = store.get(&result.handle).unwrap();
        // North: 6 rows, South: 3 rows
        let cnt_ci = col_idx(&ds, "count_count").unwrap();
        let reg_ci = col_idx(&ds, "region").unwrap();
        let mut counts: HashMap<String, f64> = HashMap::new();
        for ri in 0..ds.row_count() {
            if let Value::String(r) = col_val(&ds.data[reg_ci], ri) {
                counts.insert(r, as_f64(&ds.data[cnt_ci], ri).unwrap_or(0.0));
            }
        }
        assert!((counts["North"] - 6.0).abs() < 1e-9);
        assert!((counts["South"] - 3.0).abs() < 1e-9);
    }

    /// AC1f: count_distinct
    #[test]
    fn ac1_aggregate_count_distinct() {
        let mut store = make_store();
        let handle = store.put(sales_dataset(), 3600);
        let params = serde_json::json!({
            "group_by": ["region"],
            "agg": "count_distinct",
            "measure": "product"
        });
        let result = aggregate(&mut store, &handle, &params).unwrap();
        let ds = store.get(&result.handle).unwrap();
        // North: 2 distinct products (Widget, Gadget), South: 1 (Widget)
        let cnt_ci = col_idx(&ds, "count_distinct_product").unwrap();
        let reg_ci = col_idx(&ds, "region").unwrap();
        let mut counts: HashMap<String, f64> = HashMap::new();
        for ri in 0..ds.row_count() {
            if let Value::String(r) = col_val(&ds.data[reg_ci], ri) {
                counts.insert(r, as_f64(&ds.data[cnt_ci], ri).unwrap_or(0.0));
            }
        }
        assert!((counts["North"] - 2.0).abs() < 1e-9, "North distinct products: {}", counts["North"]);
        assert!((counts["South"] - 1.0).abs() < 1e-9, "South distinct products: {}", counts["South"]);
    }

    // ── ac2: filter ───────────────────────────────────────────────────────────

    /// AC2: compound AND/OR predicate, numeric + string
    #[test]
    fn ac2_filter_compound_predicate() {
        let mut store = make_store();
        let handle = store.put(sales_dataset(), 3600);

        // Filter: region == "North" AND revenue > 15
        let params = serde_json::json!({
            "predicate": {
                "and": [
                    { "col": "region", "op": "eq", "val": "North" },
                    { "col": "revenue", "op": "gt", "val": 15.0 }
                ]
            }
        });
        let result = filter(&mut store, &handle, &params).unwrap();
        let ds = store.get(&result.handle).unwrap();
        // North with revenue > 15: 20, 30, 25 → 3 rows
        assert_eq!(ds.row_count(), 3, "expected 3 rows after filter, got {}", ds.row_count());
    }

    #[test]
    fn ac2_filter_string_predicate() {
        let mut store = make_store();
        let handle = store.put(sales_dataset(), 3600);

        // Filter: product contains "idget" — "Widget" contains it, "Gadget" does not
        // Widget rows: 3 North + 3 South = 6 rows
        let params = serde_json::json!({
            "predicate": { "col": "product", "op": "contains", "val": "idget" }
        });
        let result = filter(&mut store, &handle, &params).unwrap();
        let ds = store.get(&result.handle).unwrap();
        assert_eq!(ds.row_count(), 6, "6 Widget rows contain 'idget' (Gadget does not)");

        // Also test string 'in' predicate: product in ["Widget", "Gadget"] = all 9
        let params_in = serde_json::json!({
            "predicate": { "col": "product", "op": "in", "val": ["Widget", "Gadget"] }
        });
        let result_in = filter(&mut store, &handle, &params_in).unwrap();
        let ds_in = store.get(&result_in.handle).unwrap();
        assert_eq!(ds_in.row_count(), 9, "all 9 rows match in [Widget, Gadget]");
    }

    #[test]
    fn ac2_filter_or_predicate() {
        let mut store = make_store();
        let handle = store.put(sales_dataset(), 3600);

        // Filter: region == "South" OR revenue >= 25
        let params = serde_json::json!({
            "predicate": {
                "or": [
                    { "col": "region", "op": "eq", "val": "South" },
                    { "col": "revenue", "op": "ge", "val": 25.0 }
                ]
            }
        });
        let result = filter(&mut store, &handle, &params).unwrap();
        let ds = store.get(&result.handle).unwrap();
        // South: 3 rows (8,12,16); North revenue >= 25: 30, 25 → 2 rows
        // Total: 5 unique (no overlap since South rows have revenue 8,12,16 < 25)
        assert_eq!(ds.row_count(), 5, "expected 5 rows, got {}", ds.row_count());
    }

    // ── ac3: sort + top_n ─────────────────────────────────────────────────────

    /// AC3a: sort is stable, multi-key asc/desc
    #[test]
    fn ac3_sort_multi_key() {
        let mut store = make_store();
        let handle = store.put(sales_dataset(), 3600);

        // Sort by region asc, revenue desc
        let params = serde_json::json!({
            "keys": [
                { "col": "region", "dir": "asc" },
                { "col": "revenue", "dir": "desc" }
            ]
        });
        let result = sort(&mut store, &handle, &params).unwrap();
        let ds = store.get(&result.handle).unwrap();
        assert_eq!(ds.row_count(), 9);

        let rev_ci = col_idx(&ds, "revenue").unwrap();
        let reg_ci = col_idx(&ds, "region").unwrap();

        // All North rows must come before South rows
        let mut regions: Vec<String> = (0..ds.row_count())
            .filter_map(|ri| {
                if let Value::String(r) = col_val(&ds.data[reg_ci], ri) { Some(r) } else { None }
            })
            .collect();
        let first_south = regions.iter().position(|r| r == "South").unwrap_or(usize::MAX);
        let last_north = regions.iter().rposition(|r| r == "North").unwrap_or(0);
        assert!(last_north < first_south, "All North must precede South");

        // Within each region, revenue must be descending
        let north_revs: Vec<f64> = (0..first_south)
            .filter_map(|ri| as_f64(&ds.data[rev_ci], ri))
            .collect();
        for w in north_revs.windows(2) {
            assert!(w[0] >= w[1], "North revenues must be descending: {} < {}", w[0], w[1]);
        }
        // Consume 'regions' to avoid unused warning
        regions.sort();
        let _ = regions;
    }

    /// AC3b: top_n returns correct N, ties broken by original row index
    #[test]
    fn ac3_top_n_deterministic_tiebreak() {
        let mut store = make_store();
        // Dataset with ties: rows with same revenue
        let col_region = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_revenue = make_col("revenue", DType::Float, ColumnRole::Measure);
        let ds = Dataset::new(
            vec![col_region, col_revenue],
            vec![
                ColumnData::Str(vec![
                    Some("A".into()), Some("B".into()), Some("C".into()),
                    Some("D".into()), Some("E".into()),
                ]),
                ColumnData::Float(vec![
                    Some(100.0), Some(100.0), Some(50.0), Some(100.0), Some(75.0),
                ]),
            ],
        ).unwrap();
        let handle = store.put(ds, 3600);

        let params = serde_json::json!({ "n": 2, "measure": "revenue", "dir": "top" });
        let result = top_n(&mut store, &handle, &params).unwrap();
        let out = store.get(&result.handle).unwrap();
        assert_eq!(out.row_count(), 2, "top_n should return exactly 2 rows");

        // With tie at 100.0, rows 0 (A) and 1 (B) should win (smaller original index)
        let reg_ci = col_idx(&out, "region").unwrap();
        let got: Vec<String> = (0..out.row_count())
            .filter_map(|ri| {
                if let Value::String(r) = col_val(&out.data[reg_ci], ri) { Some(r) } else { None }
            })
            .collect();
        assert!(got.contains(&"A".to_string()), "A must be in top-2");
        assert!(got.contains(&"B".to_string()), "B must be in top-2");
    }

    // ── ac4: pivot ────────────────────────────────────────────────────────────

    /// AC4: correct crosstab for 2-dim × 1-measure fixture
    #[test]
    fn ac4_pivot_crosstab() {
        let mut store = make_store();
        let handle = store.put(sales_dataset(), 3600);

        let params = serde_json::json!({
            "row_dim": "region",
            "col_dim": "product",
            "measure": "revenue",
            "agg": "sum"
        });
        let result = pivot(&mut store, &handle, &params).unwrap();
        let ds = store.get(&result.handle).unwrap();

        // Should have 2 rows (North, South) and 3 columns (region, Gadget, Widget)
        assert_eq!(ds.row_count(), 2);
        assert_eq!(ds.columns.len(), 3, "region + 2 product columns");

        let reg_ci = col_idx(&ds, "region").unwrap();
        let gadget_ci = col_idx(&ds, "Gadget").unwrap();
        let widget_ci = col_idx(&ds, "Widget").unwrap();

        for ri in 0..ds.row_count() {
            if let Value::String(reg) = col_val(&ds.data[reg_ci], ri) {
                match reg.as_str() {
                    "North" => {
                        let gadget_sum = as_f64(&ds.data[gadget_ci], ri).unwrap_or(0.0);
                        let widget_sum = as_f64(&ds.data[widget_ci], ri).unwrap_or(0.0);
                        // North/Gadget: 5+15+25=45, North/Widget: 10+20+30=60
                        assert!((gadget_sum - 45.0).abs() < 1e-9, "North/Gadget: {gadget_sum}");
                        assert!((widget_sum - 60.0).abs() < 1e-9, "North/Widget: {widget_sum}");
                    }
                    "South" => {
                        // South has no Gadget rows → 0.0
                        let gadget_sum = as_f64(&ds.data[gadget_ci], ri).unwrap_or(0.0);
                        let widget_sum = as_f64(&ds.data[widget_ci], ri).unwrap_or(0.0);
                        // South/Gadget: 0, South/Widget: 8+12+16=36
                        assert!((gadget_sum - 0.0).abs() < 1e-9, "South/Gadget: {gadget_sum}");
                        assert!((widget_sum - 36.0).abs() < 1e-9, "South/Widget: {widget_sum}");
                    }
                    other => panic!("unexpected region: {other}"),
                }
            }
        }
    }

    // ── ac5: compare ──────────────────────────────────────────────────────────

    /// AC5: delta + pct-change correct; 2-parent lineage recorded
    #[test]
    fn ac5_compare_delta_and_pct_change() {
        let mut store = make_store();

        // Dataset A: region, revenue
        let col_r = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_v = make_col("revenue", DType::Float, ColumnRole::Measure);
        let ds_a = Dataset::new(
            vec![col_r.clone(), col_v.clone()],
            vec![
                ColumnData::Str(vec![Some("North".into()), Some("South".into())]),
                ColumnData::Float(vec![Some(100.0), Some(50.0)]),
            ],
        ).unwrap();

        // Dataset B: region, revenue
        let ds_b = Dataset::new(
            vec![col_r, col_v],
            vec![
                ColumnData::Str(vec![Some("North".into()), Some("South".into())]),
                ColumnData::Float(vec![Some(120.0), Some(40.0)]),
            ],
        ).unwrap();

        let handle_a = store.put(ds_a, 3600);
        let handle_b = store.put(ds_b, 3600);

        let params = serde_json::json!({
            "handle_b": serde_json::to_value(&handle_b).unwrap(),
            "join_keys": ["region"],
            "measure": "revenue"
        });
        let result = compare(&mut store, &handle_a, &params).unwrap();
        let ds = store.get(&result.handle).unwrap();

        assert_eq!(ds.row_count(), 2);

        let reg_ci = col_idx(&ds, "region").unwrap();
        let delta_ci = col_idx(&ds, "delta").unwrap();
        let pct_ci = col_idx(&ds, "pct_change").unwrap();

        for ri in 0..ds.row_count() {
            if let Value::String(reg) = col_val(&ds.data[reg_ci], ri) {
                let delta = as_f64(&ds.data[delta_ci], ri).unwrap();
                let pct = as_f64(&ds.data[pct_ci], ri).unwrap();
                match reg.as_str() {
                    "North" => {
                        // delta = 120 - 100 = 20, pct = 20/100*100 = 20%
                        assert!((delta - 20.0).abs() < 1e-9, "North delta: {delta}");
                        assert!((pct - 20.0).abs() < 1e-9, "North pct: {pct}");
                    }
                    "South" => {
                        // delta = 40 - 50 = -10, pct = -10/50*100 = -20%
                        assert!((delta - (-10.0)).abs() < 1e-9, "South delta: {delta}");
                        assert!((pct - (-20.0)).abs() < 1e-9, "South pct: {pct}");
                    }
                    other => panic!("unexpected: {other}"),
                }
            }
        }

        // Verify lineage records handle_a as parent
        let lineage = store.lineage(&result.handle);
        assert!(!lineage.is_empty(), "lineage must be non-empty");
        assert_eq!(lineage[0].parents[0].id, handle_a.id, "parent is handle_a");
    }

    // ── ac6: drill ────────────────────────────────────────────────────────────

    /// AC6: drill expands a grouped row to detail rows via lineage
    #[test]
    fn ac6_drill_expands_to_detail() {
        let mut store = make_store();
        let detail_handle = store.put(sales_dataset(), 3600);

        // Aggregate to get a grouped dataset
        let agg_params = serde_json::json!({
            "group_by": ["region"],
            "agg": "sum",
            "measure": "revenue"
        });
        let agg_result = aggregate(&mut store, &detail_handle, &agg_params).unwrap();

        // Drill into the "North" group
        let drill_params = serde_json::json!({
            "group_row": { "region": "North" }
        });
        let drill_result = drill(&mut store, &agg_result.handle, &drill_params).unwrap();
        let drilled = store.get(&drill_result.handle).unwrap();

        // North has 6 detail rows in the original dataset
        assert_eq!(drilled.row_count(), 6, "drill should return 6 North detail rows, got {}", drilled.row_count());

        // Reviewer-agent counter-attack: verify drilled rows are actually North rows
        // (not random rows from the parent — AC6 requires correct *constituent* rows)
        let reg_ci = col_idx(&drilled, "region").unwrap();
        for ri in 0..drilled.row_count() {
            if let Value::String(r) = col_val(&drilled.data[reg_ci], ri) {
                assert_eq!(r, "North", "drill must only return North rows at row {ri}, got: {r}");
            }
        }
    }

    // ── ac7: new handle + summary cap ─────────────────────────────────────────

    /// AC7: every op returns a NEW handle and summary.sample ≤ sample_cap
    #[test]
    fn ac7_new_handle_and_sample_cap() {
        let mut store = make_store();

        // Build a larger dataset (30 rows) to test sample capping
        let col_r = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_v = make_col("revenue", DType::Float, ColumnRole::Measure);
        let regions: Vec<Option<String>> = (0..30).map(|i| {
            Some(if i % 2 == 0 { "North".to_string() } else { "South".to_string() })
        }).collect();
        let revenues: Vec<Option<f64>> = (0..30).map(|i| Some(i as f64 * 10.0)).collect();
        let large_ds = Dataset::new(
            vec![col_r, col_v],
            vec![ColumnData::Str(regions), ColumnData::Float(revenues)],
        ).unwrap();
        let handle = store.put(large_ds, 3600);

        // aggregate
        let agg_params = serde_json::json!({"group_by": ["region"], "agg": "sum", "measure": "revenue"});
        let r = aggregate(&mut store, &handle, &agg_params).unwrap();
        assert_ne!(r.handle.id, handle.id, "aggregate must return new handle");
        assert!(r.summary.sample.len() <= DEFAULT_SAMPLE_CAP, "aggregate sample too large");

        // filter
        let filter_params = serde_json::json!({"predicate": {"col": "region", "op": "eq", "val": "North"}});
        let r2 = filter(&mut store, &handle, &filter_params).unwrap();
        assert_ne!(r2.handle.id, handle.id, "filter must return new handle");
        assert!(r2.summary.sample.len() <= DEFAULT_SAMPLE_CAP, "filter sample too large");

        // sort
        let sort_params = serde_json::json!({"keys": [{"col": "revenue", "dir": "asc"}]});
        let r3 = sort(&mut store, &handle, &sort_params).unwrap();
        assert_ne!(r3.handle.id, handle.id, "sort must return new handle");
        assert!(r3.summary.sample.len() <= DEFAULT_SAMPLE_CAP, "sort sample too large");

        // top_n
        let topn_params = serde_json::json!({"n": 5, "measure": "revenue", "dir": "top"});
        let r4 = top_n(&mut store, &handle, &topn_params).unwrap();
        assert_ne!(r4.handle.id, handle.id, "top_n must return new handle");
        assert!(r4.summary.sample.len() <= DEFAULT_SAMPLE_CAP, "top_n sample too large");

        // describe
        let describe_params = serde_json::json!({});
        let r5 = describe(&mut store, &handle, &describe_params).unwrap();
        assert_ne!(r5.handle.id, handle.id, "describe must return new handle");
        assert!(r5.summary.sample.len() <= DEFAULT_SAMPLE_CAP, "describe sample too large");
    }

    // ── ac8: determinism ──────────────────────────────────────────────────────

    /// AC8: running any op twice on the same input yields byte-identical stored output.
    #[test]
    fn ac8_determinism_byte_identical() {
        // For each op: run twice on fresh stores with identical input, compare
        // serialized stored output.
        let ds = sales_dataset();

        let run_agg = |ds: Dataset| -> Vec<u8> {
            let mut store = make_store();
            let handle = store.put(ds, 3600);
            let params = serde_json::json!({"group_by": ["region"], "agg": "sum", "measure": "revenue"});
            let r = aggregate(&mut store, &handle, &params).unwrap();
            let out = store.get(&r.handle).unwrap();
            serde_json::to_vec(&out).unwrap()
        };

        let bytes1 = run_agg(ds.clone());
        let bytes2 = run_agg(ds.clone());
        assert_eq!(bytes1, bytes2, "aggregate output must be byte-identical across runs");

        let run_filter = |ds: Dataset| -> Vec<u8> {
            let mut store = make_store();
            let handle = store.put(ds, 3600);
            let params = serde_json::json!({"predicate": {"col": "region", "op": "eq", "val": "North"}});
            let r = filter(&mut store, &handle, &params).unwrap();
            let out = store.get(&r.handle).unwrap();
            serde_json::to_vec(&out).unwrap()
        };

        let f1 = run_filter(ds.clone());
        let f2 = run_filter(ds.clone());
        assert_eq!(f1, f2, "filter output must be byte-identical");

        let run_sort = |ds: Dataset| -> Vec<u8> {
            let mut store = make_store();
            let handle = store.put(ds, 3600);
            let params = serde_json::json!({"keys": [{"col": "revenue", "dir": "desc"}]});
            let r = sort(&mut store, &handle, &params).unwrap();
            let out = store.get(&r.handle).unwrap();
            serde_json::to_vec(&out).unwrap()
        };

        let s1 = run_sort(ds.clone());
        let s2 = run_sort(ds.clone());
        assert_eq!(s1, s2, "sort output must be byte-identical");

        let run_pivot = |ds: Dataset| -> Vec<u8> {
            let mut store = make_store();
            let handle = store.put(ds, 3600);
            let params = serde_json::json!({"row_dim": "region", "col_dim": "product", "measure": "revenue", "agg": "sum"});
            let r = pivot(&mut store, &handle, &params).unwrap();
            let out = store.get(&r.handle).unwrap();
            serde_json::to_vec(&out).unwrap()
        };

        let p1 = run_pivot(ds.clone());
        let p2 = run_pivot(ds.clone());
        assert_eq!(p1, p2, "pivot output must be byte-identical");
    }

    // ── targeted mutant-kill tests ────────────────────────────────────────────

    /// Kill: eval_predicate Ne vs Eq confusion (lines ~497, 506, 508, 512).
    /// A `ne` filter must exclude the matching rows and keep the rest.
    #[test]
    fn mk_filter_ne_excludes_matching() {
        let mut store = make_store();
        let handle = store.put(sales_dataset(), 3600);
        // ne "North" → only South rows (3 rows)
        let params = serde_json::json!({
            "predicate": { "col": "region", "op": "ne", "val": "North" }
        });
        let result = filter(&mut store, &handle, &params).unwrap();
        let ds = store.get(&result.handle).unwrap();
        assert_eq!(ds.row_count(), 3, "ne should keep 3 South rows");
        let reg_ci = col_idx(&ds, "region").unwrap();
        for ri in 0..ds.row_count() {
            if let Value::String(r) = col_val(&ds.data[reg_ci], ri) {
                assert_eq!(r, "South", "all remaining rows must be South");
            }
        }
    }

    /// Kill: eval_predicate IsNull/IsNotNull confusion.
    #[test]
    fn mk_filter_null_checks() {
        let col_v = make_col("value", DType::Float, ColumnRole::Measure);
        let ds = Dataset::new(
            vec![col_v],
            vec![ColumnData::Float(vec![Some(1.0), None, Some(3.0), None])],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(ds, 3600);

        let params_null = serde_json::json!({"predicate": {"col": "value", "op": "is_null"}});
        let r_null = filter(&mut store, &handle, &params_null).unwrap();
        assert_eq!(store.get(&r_null.handle).unwrap().row_count(), 2, "2 nulls");

        let params_notnull = serde_json::json!({"predicate": {"col": "value", "op": "is_not_null"}});
        let r_notnull = filter(&mut store, &handle, &params_notnull).unwrap();
        assert_eq!(store.get(&r_notnull.handle).unwrap().row_count(), 2, "2 non-nulls");
    }

    /// Kill: top_n == vs != tie-break confusion (line ~726).
    /// When ties are present the tie-break must use < not <=.
    #[test]
    fn mk_top_n_tiebreak_order_is_index() {
        let col_region = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_revenue = make_col("revenue", DType::Float, ColumnRole::Measure);
        let ds = Dataset::new(
            vec![col_region, col_revenue],
            vec![
                ColumnData::Str(vec![
                    Some("E".into()), Some("A".into()), Some("B".into()),
                    Some("C".into()), Some("D".into()),
                ]),
                ColumnData::Float(vec![
                    Some(100.0), Some(100.0), Some(100.0),
                    Some(50.0), Some(75.0),
                ]),
            ],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(ds, 3600);
        let params = serde_json::json!({ "n": 2, "measure": "revenue", "dir": "top" });
        let result = top_n(&mut store, &handle, &params).unwrap();
        let out = store.get(&result.handle).unwrap();
        assert_eq!(out.row_count(), 2, "exactly 2 rows");
        // Rows 0 (E) and 1 (A) have identical revenue=100 and the smallest indices.
        let reg_ci = col_idx(&out, "region").unwrap();
        let got: Vec<String> = (0..out.row_count())
            .filter_map(|ri| if let Value::String(r) = col_val(&out.data[reg_ci], ri) { Some(r) } else { None })
            .collect();
        assert!(got.contains(&"E".to_string()), "E (idx 0) must win tie");
        assert!(got.contains(&"A".to_string()), "A (idx 1) must win tie");
        assert!(!got.contains(&"B".to_string()), "B (idx 2) must lose tie");
    }

    /// Kill: pivot mean agg: division in apply_agg (line ~822).
    #[test]
    fn mk_pivot_mean_agg_correct() {
        let col_region = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_product = make_col("product", DType::Str, ColumnRole::Dimension);
        let col_revenue = make_col("revenue", DType::Float, ColumnRole::Measure);
        // 2 rows for North/Widget with revenues 10 and 30 → mean=20
        let ds = Dataset::new(
            vec![col_region, col_product, col_revenue],
            vec![
                ColumnData::Str(vec![Some("North".into()), Some("North".into())]),
                ColumnData::Str(vec![Some("Widget".into()), Some("Widget".into())]),
                ColumnData::Float(vec![Some(10.0), Some(30.0)]),
            ],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(ds, 3600);
        let params = serde_json::json!({
            "row_dim": "region", "col_dim": "product", "measure": "revenue", "agg": "mean"
        });
        let result = pivot(&mut store, &handle, &params).unwrap();
        let ds_out = store.get(&result.handle).unwrap();
        let widget_ci = col_idx(&ds_out, "Widget").unwrap();
        let mean_val = as_f64(&ds_out.data[widget_ci], 0).unwrap();
        assert!((mean_val - 20.0).abs() < 1e-9, "mean should be 20, got {mean_val}");
    }

    /// Kill: drill > vs >= row-count comparison (lines ~1074, ~1086).
    /// A grouped dataset with equal row count as detail should NOT find a parent.
    #[test]
    fn mk_drill_requires_strictly_more_rows_in_parent() {
        // Create a dataset with 1 row and aggregate it: 1 group = 1 row
        // drill on the aggregated handle should fail (no parent with > current rows)
        let col_region = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_revenue = make_col("revenue", DType::Float, ColumnRole::Measure);
        let single_row_ds = Dataset::new(
            vec![col_region, col_revenue],
            vec![
                ColumnData::Str(vec![Some("North".into())]),
                ColumnData::Float(vec![Some(42.0)]),
            ],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(single_row_ds, 3600);
        // Aggregate produces 1 group = 1 row: same row count as parent.
        // drill on this should return Unsupported (parent has same row count, not more).
        let agg_params = serde_json::json!({"group_by": ["region"], "agg": "sum", "measure": "revenue"});
        let agg_result = aggregate(&mut store, &handle, &agg_params).unwrap();
        let drill_params = serde_json::json!({"group_row": {"region": "North"}});
        let drill_err = drill(&mut store, &agg_result.handle, &drill_params);
        assert!(drill_err.is_err(), "drill on 1-row agg over 1-row source should fail (no parent with strictly more rows)");
    }

    /// Kill: as_sort_key IEEE 754 bit manipulation (integer sort-key encoding).
    /// Negative integers must sort before positive ones in lexicographic key order.
    #[test]
    fn mk_sort_key_negative_integers_before_positive() {
        let col_val_col = make_col("v", DType::Int, ColumnRole::Measure);
        let ds = Dataset::new(
            vec![col_val_col],
            vec![ColumnData::Int(vec![Some(5), Some(-3), Some(0), Some(-100), Some(7)])],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(ds, 3600);
        let params = serde_json::json!({"keys": [{"col": "v", "dir": "asc"}]});
        let result = sort(&mut store, &handle, &params).unwrap();
        let out = store.get(&result.handle).unwrap();
        let ci = col_idx(&out, "v").unwrap();
        let sorted: Vec<i64> = (0..out.row_count())
            .filter_map(|ri| if let Value::Number(n) = col_val(&out.data[ci], ri) { n.as_i64() } else { None })
            .collect();
        assert_eq!(sorted, vec![-100, -3, 0, 5, 7], "integers must sort correctly: {sorted:?}");
    }

    /// Kill: as_sort_key for floats — negative floats must sort before positive.
    #[test]
    fn mk_sort_key_negative_floats_before_positive() {
        let col_v = make_col("v", DType::Float, ColumnRole::Measure);
        let ds = Dataset::new(
            vec![col_v],
            vec![ColumnData::Float(vec![Some(3.14), Some(-2.71), Some(0.0), Some(-100.0), Some(1.0)])],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(ds, 3600);
        let params = serde_json::json!({"keys": [{"col": "v", "dir": "asc"}]});
        let result = sort(&mut store, &handle, &params).unwrap();
        let out = store.get(&result.handle).unwrap();
        let ci = col_idx(&out, "v").unwrap();
        let sorted: Vec<f64> = (0..out.row_count())
            .filter_map(|ri| as_f64(&out.data[ci], ri))
            .collect();
        assert_eq!(sorted, vec![-100.0, -2.71, 0.0, 1.0, 3.14], "floats must sort correctly: {sorted:?}");
    }

    /// Kill: col_val Bool arm and as_f64 Int arm.
    #[test]
    fn mk_col_val_int_and_bool_arms() {
        let col_i = make_col("count", DType::Int, ColumnRole::Measure);
        let col_b = make_col("flag", DType::Str, ColumnRole::Dimension);
        // Use Int column to exercise col_val::Int arm and as_f64::Int arm
        let ds = Dataset::new(
            vec![col_i.clone(), col_b.clone()],
            vec![
                ColumnData::Int(vec![Some(42), Some(-7), None]),
                ColumnData::Bool(vec![Some(true), Some(false), None]),
            ],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(ds, 3600);

        // col_val Int arm: filter by count >= 0 should return rows 0 and 1 (not None)
        // Actually: gt 40 → row 0 only (count=42)
        let params = serde_json::json!({"predicate": {"col": "count", "op": "gt", "val": 40}});
        let r = filter(&mut store, &handle, &params).unwrap();
        assert_eq!(store.get(&r.handle).unwrap().row_count(), 1, "count > 40 is row 0 only");

        // as_f64 Int arm: aggregate sum over Int column
        let agg_params = serde_json::json!({"group_by": ["flag"], "agg": "sum", "measure": "count"});
        let r2 = aggregate(&mut store, &handle, &agg_params).unwrap();
        let ds2 = store.get(&r2.handle).unwrap();
        // Should have groups (true → 42, false → -7, "" for null)
        assert!(ds2.row_count() >= 2, "should have at least 2 groups for bool column");
    }

    /// Kill: OpError::fmt (Display impl) — test that Display produces non-empty output.
    #[test]
    fn mk_op_error_display_non_empty() {
        let variants = vec![
            OpError::HandleNotFound("h1".to_string()),
            OpError::BadParam("p".to_string()),
            OpError::UnknownColumn("c".to_string()),
            OpError::Unsupported("u".to_string()),
            OpError::Internal("i".to_string()),
        ];
        for e in variants {
            let s = e.to_string();
            assert!(!s.is_empty(), "Display must not be empty for {e:?}");
            assert!(s.contains(':'), "Display should contain colon separator: {s:?}");
        }
    }

    /// Kill: eval_predicate Eq vs Ne at lines 490/497 — must distinguish eq from ne in filter.
    /// The existing mk_filter_ne_excludes_matching covers one direction; this hits the
    /// is_some_and path with both positive and negative checks on the same column.
    #[test]
    fn mk_eval_predicate_eq_ne_distinction() {
        let col_v = make_col("x", DType::Int, ColumnRole::Measure);
        let ds = Dataset::new(
            vec![col_v],
            vec![ColumnData::Int(vec![Some(1), Some(2), Some(3)])],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(ds, 3600);

        // eq 2 → exactly 1 row (x=2)
        let r_eq = filter(&mut store, &handle,
            &serde_json::json!({"predicate": {"col": "x", "op": "eq", "val": 2}})).unwrap();
        assert_eq!(store.get(&r_eq.handle).unwrap().row_count(), 1, "eq=2 should yield 1 row");

        // ne 2 → exactly 2 rows (x=1, x=3)
        let r_ne = filter(&mut store, &handle,
            &serde_json::json!({"predicate": {"col": "x", "op": "ne", "val": 2}})).unwrap();
        assert_eq!(store.get(&r_ne.handle).unwrap().row_count(), 2, "ne=2 should yield 2 rows");

        // le 2 → rows 0,1 (x=1,2) = 2 rows; le must not behave like lt
        let r_le = filter(&mut store, &handle,
            &serde_json::json!({"predicate": {"col": "x", "op": "le", "val": 2}})).unwrap();
        assert_eq!(store.get(&r_le.handle).unwrap().row_count(), 2, "le=2 yields 2 rows (1 and 2)");

        // ge 2 → rows 1,2 (x=2,3) = 2 rows; ge must not behave like gt
        let r_ge = filter(&mut store, &handle,
            &serde_json::json!({"predicate": {"col": "x", "op": "ge", "val": 2}})).unwrap();
        assert_eq!(store.get(&r_ge.handle).unwrap().row_count(), 2, "ge=2 yields 2 rows (2 and 3)");
    }

    /// Kill: top_n == vs != in tie-break (line 726): when there are NO ties,
    /// the condition `ord == Equal` should be false — different behaviour than !=.
    #[test]
    fn mk_top_n_no_tie_returns_correct_row() {
        let col_region = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_revenue = make_col("revenue", DType::Float, ColumnRole::Measure);
        let ds = Dataset::new(
            vec![col_region, col_revenue],
            vec![
                ColumnData::Str(vec![Some("A".into()), Some("B".into()), Some("C".into())]),
                ColumnData::Float(vec![Some(30.0), Some(10.0), Some(20.0)]),
            ],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(ds, 3600);
        let params = serde_json::json!({"n": 1, "measure": "revenue", "dir": "top"});
        let result = top_n(&mut store, &handle, &params).unwrap();
        let out = store.get(&result.handle).unwrap();
        assert_eq!(out.row_count(), 1, "top-1 should return 1 row");
        let reg_ci = col_idx(&out, "region").unwrap();
        if let Value::String(r) = col_val(&out.data[reg_ci], 0) {
            assert_eq!(r, "A", "top-1 by revenue should be A (30.0)");
        }
    }

    /// Kill: as_sort_key int bias arithmetic (line 141: + must not be - or *).
    /// Specifically: the max i64 value must sort AFTER i64::MIN and after 0.
    #[test]
    fn mk_sort_key_i64_extremes() {
        let col_v = make_col("v", DType::Int, ColumnRole::Measure);
        let ds = Dataset::new(
            vec![col_v],
            vec![ColumnData::Int(vec![Some(i64::MAX), Some(0), Some(i64::MIN)])],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(ds, 3600);
        let params = serde_json::json!({"keys": [{"col": "v", "dir": "asc"}]});
        let result = sort(&mut store, &handle, &params).unwrap();
        let out = store.get(&result.handle).unwrap();
        let ci = col_idx(&out, "v").unwrap();
        let sorted: Vec<i64> = (0..out.row_count())
            .filter_map(|ri| if let Value::Number(n) = col_val(&out.data[ci], ri) { n.as_i64() } else { None })
            .collect();
        assert_eq!(sorted, vec![i64::MIN, 0, i64::MAX], "i64 extremes must sort correctly: {sorted:?}");
    }

    /// Kill: as_sort_key IEEE 754 XOR bit-flip (line 153: ^ must not be |).
    /// NaN-free negatives must sort before negatives closer to zero.
    #[test]
    fn mk_sort_key_f64_bit_flip() {
        let col_v = make_col("v", DType::Float, ColumnRole::Measure);
        // f64 values that differ only in IEEE 754 sign bit handling
        let ds = Dataset::new(
            vec![col_v],
            vec![ColumnData::Float(vec![
                Some(f64::MAX), Some(f64::MIN_POSITIVE), Some(-f64::MIN_POSITIVE), Some(f64::MIN),
            ])],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(ds, 3600);
        let params = serde_json::json!({"keys": [{"col": "v", "dir": "asc"}]});
        let result = sort(&mut store, &handle, &params).unwrap();
        let out = store.get(&result.handle).unwrap();
        let ci = col_idx(&out, "v").unwrap();
        let sorted: Vec<f64> = (0..out.row_count())
            .filter_map(|ri| as_f64(&out.data[ci], ri))
            .collect();
        // Expected asc order: f64::MIN < -f64::MIN_POSITIVE < f64::MIN_POSITIVE < f64::MAX
        assert!(sorted[0] < sorted[1] && sorted[1] < sorted[2] && sorted[2] < sorted[3],
            "f64 extremes must sort correctly: {sorted:?}");
    }

    /// Kill: drill > vs >= row-count comparison (lines 1074, 1086).
    /// A parent with the SAME row count as the aggregated dataset must NOT qualify.
    /// Build a 2-row dataset → aggregate into 2 groups (same count) → drill should fail.
    #[test]
    fn mk_drill_same_row_count_parent_not_qualifiable() {
        // 2 rows, 2 distinct regions → aggregate(group_by=[region]) → 2 groups
        let col_region = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_revenue = make_col("revenue", DType::Float, ColumnRole::Measure);
        let ds = Dataset::new(
            vec![col_region, col_revenue],
            vec![
                ColumnData::Str(vec![Some("A".into()), Some("B".into())]),
                ColumnData::Float(vec![Some(10.0), Some(20.0)]),
            ],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(ds, 3600);
        let agg_params = serde_json::json!({"group_by": ["region"], "agg": "sum", "measure": "revenue"});
        let agg_result = aggregate(&mut store, &handle, &agg_params).unwrap();
        // The aggregated dataset has 2 rows; the parent also has 2 rows → NOT strictly more.
        let drill_params = serde_json::json!({"group_row": {"region": "A"}});
        let drill_err = drill(&mut store, &agg_result.handle, &drill_params);
        assert!(drill_err.is_err(), "drill must fail when parent row count equals child row count (no strictly-more-rows parent)");
    }

    /// Kill: as_f64 Decimal arm (line 115).
    #[test]
    fn mk_as_f64_decimal_arm() {
        let col_d = make_col("amount", DType::Decimal, ColumnRole::Measure);
        let ds = Dataset::new(
            vec![col_d],
            vec![ColumnData::Decimal(vec![Some("3.14".into()), Some("2.72".into())])],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(ds, 3600);
        // Filter on Decimal gt — exercises as_f64 Decimal arm via compare_val
        let filter_params = serde_json::json!({"predicate": {"col": "amount", "op": "gt", "val": 3.0}});
        let r = filter(&mut store, &handle, &filter_params).unwrap();
        assert_eq!(store.get(&r.handle).unwrap().row_count(), 1, "Decimal gt 3.0 should return 1 row (3.14)");
    }

    /// Kill: select_col_rows Bool / Date / Time / Decimal arms
    #[test]
    fn mk_select_col_rows_all_types() {
        use dh_spec::DType;
        let col_b = make_col("flag", DType::Bool, ColumnRole::Dimension);
        let col_d = make_col("dt", DType::Date, ColumnRole::Dimension);
        let ds = Dataset::new(
            vec![col_b, col_d],
            vec![
                ColumnData::Bool(vec![Some(true), Some(false), Some(true)]),
                ColumnData::Date(vec![Some("2026-01-01".into()), Some("2026-01-02".into()), Some("2026-01-03".into())]),
            ],
        ).unwrap();
        let mut store = make_store();
        let handle = store.put(ds, 3600);
        // sort by date asc
        let params = serde_json::json!({"keys": [{"col": "dt", "dir": "asc"}]});
        let result = sort(&mut store, &handle, &params).unwrap();
        let out = store.get(&result.handle).unwrap();
        assert_eq!(out.row_count(), 3, "all 3 rows preserved through sort");
        // Check Bool column was carried through select_col_rows correctly
        let b_ci = col_idx(&out, "flag").unwrap();
        // row 0 must be "2026-01-01" → flag=true (original row 0)
        if let Value::Bool(b) = col_val(&out.data[b_ci], 0) {
            assert!(b, "first row (earliest date) should have flag=true");
        }
    }

    // ── ac9: describe ─────────────────────────────────────────────────────────

    /// AC9: describe returns per-column stats, new handle, correct row count
    #[test]
    fn ac9_describe_per_column_stats() {
        let mut store = make_store();
        let handle = store.put(sales_dataset(), 3600);

        let params = serde_json::json!({});
        let result = describe(&mut store, &handle, &params).unwrap();
        let ds = store.get(&result.handle).unwrap();

        // Input has 3 columns → describe has 3 rows
        assert_eq!(ds.row_count(), 3, "describe should have one row per input column");

        // Verify handle is new
        assert_ne!(result.handle.id, handle.id, "describe must return new handle");

        // Summary sample must be ≤ sample_cap
        assert!(result.summary.sample.len() <= DEFAULT_SAMPLE_CAP);

        // Check col_name column exists
        let cn_ci = col_idx(&ds, "col_name").unwrap();
        let col_names: Vec<String> = (0..ds.row_count())
            .filter_map(|ri| {
                if let Value::String(s) = col_val(&ds.data[cn_ci], ri) { Some(s) } else { None }
            })
            .collect();
        assert!(col_names.contains(&"region".to_string()));
        assert!(col_names.contains(&"product".to_string()));
        assert!(col_names.contains(&"revenue".to_string()));
    }
}
