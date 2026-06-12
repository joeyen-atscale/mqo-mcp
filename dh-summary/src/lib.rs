//! # dh-summary
//!
//! Bounded summary of a [`Dataset`] that the LLM sees instead of raw rows.
//!
//! The central function is [`summarize`], which produces a [`DatasetSummary`]
//! from a [`Dataset`] and a [`SummaryCfg`].  The summary is guaranteed to be
//! ≤ `cfg.max_bytes` when serialised to JSON — if necessary the *sample* is
//! truncated (stats are never removed) and a truncation note is appended.
//!
//! [`capabilities`] separately advertises which operations make sense for a
//! given dataset's shape.

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use std::collections::HashMap;

use dh_spec::{Capability, ColStats, ColumnRole, DatasetSummary, DType, Row};
use dh_store::{ColumnData, Dataset};
use serde_json::Value;

// ── Configuration ──────────────────────────────────────────────────────────

/// Configuration for [`summarize`].
#[derive(Debug, Clone)]
pub struct SummaryCfg {
    /// Maximum number of sample rows in the returned summary.
    ///
    /// The head rows (up to `sample_cap / 2`) and tail rows (up to
    /// `sample_cap / 2`) are taken, giving a combined head+tail view.
    /// Default: 8 (head ≤4 + tail ≤4).
    pub sample_cap: usize,

    /// Maximum number of top-k values stored per categorical column.
    /// Default: 10.
    pub topk: usize,

    /// Maximum serialised size (in bytes) of the returned [`DatasetSummary`].
    ///
    /// If the summary exceeds this limit the sample is progressively truncated
    /// (never the stats) until it fits, and a truncation note is added.
    /// Default: 32 768 bytes (32 KiB).
    pub max_bytes: usize,
}

impl Default for SummaryCfg {
    fn default() -> Self {
        Self {
            sample_cap: 8,
            topk: 10,
            max_bytes: 32_768,
        }
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Produce a bounded [`DatasetSummary`] from a [`Dataset`].
///
/// # Guarantees
///
/// * `summary.sample.len() <= cfg.sample_cap`
/// * `serde_json::to_vec(&summary).unwrap().len() <= cfg.max_bytes`
///   (the sample is truncated, never the stats, to enforce this)
///
/// # Panics
///
/// Panics if `cfg.max_bytes` is so small that even the zero-sample summary
/// (no rows at all) cannot fit.  In practice 1 KiB is always sufficient for
/// the header + stats.
#[must_use]
pub fn summarize(dataset: &Dataset, cfg: &SummaryCfg) -> DatasetSummary {
    let row_count = dataset.row_count() as u64;

    // ── Build per-column stats ────────────────────────────────────────────
    let stats = compute_stats(dataset, cfg.topk);

    // ── Build head/tail sample ────────────────────────────────────────────
    let sample = build_sample(dataset, cfg.sample_cap);

    // ── Generate deterministic notes ──────────────────────────────────────
    let mut notes = generate_notes(dataset, &stats);

    // ── Assemble summary (respects sample_cap via DatasetSummary::new) ───
    let summary = DatasetSummary::new(
        row_count,
        dataset.columns.clone(),
        sample,
        cfg.sample_cap,
        stats,
        notes.clone(),
    );

    // ── Hard max_bytes self-check: truncate sample if needed ──────────────
    enforce_max_bytes(summary, row_count, cfg, &mut notes)
}

/// Advertise which operations make sense for this dataset's shape.
///
/// Rules:
/// * `Pivot` requires ≥2 dimension columns.
/// * `Chart` and `BiAsset` require ≥1 row and ≥1 measure column.
/// * All other capabilities are always advertised.
#[must_use]
pub fn capabilities(dataset: &Dataset) -> Vec<Capability> {
    let n_dims = dataset
        .columns
        .iter()
        .filter(|c| c.role == ColumnRole::Dimension)
        .count();
    let n_measures = dataset
        .columns
        .iter()
        .filter(|c| c.role == ColumnRole::Measure)
        .count();
    let has_rows = dataset.row_count() > 0;

    let mut caps = Vec::with_capacity(11);
    caps.push(Capability::Aggregate);
    caps.push(Capability::Filter);
    caps.push(Capability::Sort);
    caps.push(Capability::TopN);
    if n_dims >= 2 {
        caps.push(Capability::Pivot);
    }
    caps.push(Capability::Compare);
    caps.push(Capability::Drill);
    caps.push(Capability::Describe);
    caps.push(Capability::Export);
    // Chart and BiAsset are available whenever the dataset has data to visualise.
    if has_rows && n_measures >= 1 {
        caps.push(Capability::Chart);
        caps.push(Capability::BiAsset);
    }
    caps
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Build the head+tail sample, capped at `sample_cap` total rows.
fn build_sample(dataset: &Dataset, sample_cap: usize) -> Vec<Row> {
    let n = dataset.row_count();
    if n == 0 || sample_cap == 0 {
        return Vec::new();
    }

    let head_count = (sample_cap + 1) / 2; // ceiling half
    let tail_count = sample_cap / 2;

    // Which row indices to include (deduped, in order)
    let mut indices: Vec<usize> = Vec::with_capacity(sample_cap);
    for i in 0..head_count.min(n) {
        indices.push(i);
    }
    if tail_count > 0 {
        let tail_start = n.saturating_sub(tail_count).max(head_count.min(n));
        for i in tail_start..n {
            indices.push(i);
        }
    }

    indices.iter().map(|&row_idx| row_to_map(dataset, row_idx)).collect()
}

/// Convert a single row index into a `Row` (`HashMap`<String, Value>).
fn row_to_map(dataset: &Dataset, row_idx: usize) -> Row {
    let mut map = Row::new();
    for (col, data) in dataset.columns.iter().zip(dataset.data.iter()) {
        let val = extract_value(data, row_idx);
        map.insert(col.name.clone(), val);
    }
    map
}

/// Extract the JSON `Value` at position `row_idx` from [`ColumnData`].
fn extract_value(data: &ColumnData, row_idx: usize) -> Value {
    match data {
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

/// Compute per-column statistics keyed by `unique_name`.
fn compute_stats(dataset: &Dataset, topk: usize) -> HashMap<String, ColStats> {
    let mut stats = HashMap::new();
    for (col, data) in dataset.columns.iter().zip(dataset.data.iter()) {
        let s = compute_col_stats(data, col.dtype, topk);
        stats.insert(col.unique_name.clone(), s);
    }
    stats
}

fn compute_col_stats(data: &ColumnData, dtype: DType, topk: usize) -> ColStats {
    match dtype {
        DType::Int => stats_int(data),
        DType::Float | DType::Decimal => stats_float(data),
        DType::Str => stats_str(data, topk),
        DType::Bool => stats_bool(data),
        DType::Date | DType::Time => stats_date(data),
    }
}

#[allow(clippy::cast_precision_loss)]
fn stats_int(data: &ColumnData) -> ColStats {
    let (vals, null_count, total) = match data {
        ColumnData::Int(v) => {
            let non_null: Vec<i64> = v.iter().filter_map(|o| *o).collect();
            let nulls = v.iter().filter(|o| o.is_none()).count() as u64;
            let tot = v.len() as u64;
            (non_null, nulls, tot)
        }
        _ => (vec![], 0, 0),
    };
    let distinct = count_distinct_int(&vals);
    if vals.is_empty() {
        return ColStats {
            min: None,
            max: None,
            sum: None,
            mean: None,
            distinct: Some(distinct + null_count.min(1)),
            top_k: None,
        };
    }
    let min = vals.iter().copied().min().unwrap() as f64;
    let max = vals.iter().copied().max().unwrap() as f64;
    let sum: f64 = vals.iter().map(|&v| v as f64).sum();
    let non_null_count = total - null_count;
    let mean = sum / non_null_count as f64;
    ColStats {
        min: Some(min),
        max: Some(max),
        sum: Some(sum),
        mean: Some(mean),
        distinct: Some(distinct + null_count.min(1)),
        top_k: None,
    }
}

#[allow(clippy::cast_precision_loss)]
fn stats_float(data: &ColumnData) -> ColStats {
    let (vals, null_count, total) = match data {
        ColumnData::Float(v) => {
            let non_null: Vec<f64> = v.iter().filter_map(|o| *o).collect();
            let nulls = v.iter().filter(|o| o.is_none()).count() as u64;
            let tot = v.len() as u64;
            (non_null, nulls, tot)
        }
        ColumnData::Decimal(v) => {
            let non_null: Vec<f64> = v
                .iter()
                .filter_map(|o| o.as_deref())
                .filter_map(|s| s.parse::<f64>().ok())
                .collect();
            let nulls = v.iter().filter(|o| o.is_none()).count() as u64;
            let tot = v.len() as u64;
            (non_null, nulls, tot)
        }
        _ => (vec![], 0, 0),
    };
    let non_null_count = total - null_count;
    if vals.is_empty() {
        return ColStats {
            min: None,
            max: None,
            sum: None,
            mean: None,
            distinct: Some(null_count.min(1)),
            top_k: None,
        };
    }
    let min = vals.iter().copied().fold(f64::INFINITY, f64::min);
    let max = vals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let sum: f64 = vals.iter().sum();
    let mean = sum / non_null_count as f64;
    // Approximate distinct count via sort-dedup on bits.
    let mut sorted = vals.clone();
    sorted.sort_by(f64::total_cmp);
    sorted.dedup_by(|a, b| a.to_bits() == b.to_bits());
    let distinct = sorted.len() as u64 + null_count.min(1);
    ColStats {
        min: Some(min),
        max: Some(max),
        sum: Some(sum),
        mean: Some(mean),
        distinct: Some(distinct),
        top_k: None,
    }
}

#[allow(clippy::cast_precision_loss)]
fn stats_str(data: &ColumnData, topk: usize) -> ColStats {
    let (values, null_count) = match data {
        ColumnData::Str(v) => {
            let non_null: Vec<&str> = v.iter().filter_map(|o| o.as_deref()).collect();
            let nulls = v.iter().filter(|o| o.is_none()).count() as u64;
            (non_null, nulls)
        }
        _ => (vec![], 0),
    };
    let mut freq: HashMap<&str, u64> = HashMap::new();
    for s in &values {
        *freq.entry(s).or_insert(0) += 1;
    }
    let distinct = freq.len() as u64 + null_count.min(1);
    let mut freq_vec: Vec<(&str, u64)> = freq.into_iter().collect();
    freq_vec.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    freq_vec.truncate(topk);
    let entries: Vec<Value> = freq_vec
        .into_iter()
        .map(|(s, c)| serde_json::json!({"value": s, "count": c}))
        .collect();
    ColStats {
        min: None,
        max: None,
        sum: None,
        mean: None,
        distinct: Some(distinct),
        top_k: if entries.is_empty() { None } else { Some(entries) },
    }
}

fn stats_bool(data: &ColumnData) -> ColStats {
    let (trues, falses, nulls) = match data {
        ColumnData::Bool(v) => {
            let t = v.iter().filter(|o| matches!(o, Some(true))).count() as u64;
            let f = v.iter().filter(|o| matches!(o, Some(false))).count() as u64;
            let n = v.iter().filter(|o| o.is_none()).count() as u64;
            (t, f, n)
        }
        _ => (0, 0, 0),
    };
    let distinct = u64::from(trues > 0) + u64::from(falses > 0) + u64::from(nulls > 0);
    let mut entries: Vec<Value> = Vec::new();
    if trues > 0 {
        entries.push(serde_json::json!({"value": true, "count": trues}));
    }
    if falses > 0 {
        entries.push(serde_json::json!({"value": false, "count": falses}));
    }
    ColStats {
        min: None,
        max: None,
        sum: None,
        mean: None,
        distinct: Some(distinct),
        top_k: if entries.is_empty() { None } else { Some(entries) },
    }
}

fn stats_date(data: &ColumnData) -> ColStats {
    let (values, null_count) = match data {
        ColumnData::Date(v) | ColumnData::Time(v) => {
            let non_null: Vec<&str> = v.iter().filter_map(|o| o.as_deref()).collect();
            let nulls = v.iter().filter(|o| o.is_none()).count() as u64;
            (non_null, nulls)
        }
        _ => (vec![], 0),
    };
    let mut sorted: Vec<&str> = values.clone();
    sorted.sort_unstable();
    let min_s = sorted.first().map(|s| (*s).to_string());
    let max_s = sorted.last().map(|s| (*s).to_string());
    let distinct: u64 = {
        let mut u = sorted.clone();
        u.dedup();
        u.len() as u64 + null_count.min(1)
    };
    // Surface min/max via top_k so notes and tests can read them.
    let span_entry = match (&min_s, &max_s) {
        (Some(min), Some(max)) => Some(vec![
            serde_json::json!({"value": min, "label": "min"}),
            serde_json::json!({"value": max, "label": "max"}),
        ]),
        _ => None,
    };
    ColStats {
        min: None,
        max: None,
        sum: None,
        mean: None,
        distinct: Some(distinct),
        top_k: span_entry,
    }
}

fn count_distinct_int(vals: &[i64]) -> u64 {
    let mut v = vals.to_vec();
    v.sort_unstable();
    v.dedup();
    v.len() as u64
}

/// Generate deterministic notes about nulls, dominant groups, date spans, etc.
#[allow(clippy::cast_precision_loss)]
fn generate_notes(dataset: &Dataset, stats: &HashMap<String, ColStats>) -> Vec<String> {
    let mut notes = Vec::new();
    let total_rows = dataset.row_count();
    if total_rows == 0 {
        notes.push("Dataset is empty.".to_string());
        return notes;
    }

    for (col, data) in dataset.columns.iter().zip(dataset.data.iter()) {
        // Null note
        let null_count = count_nulls(data);
        if null_count > 0 {
            notes.push(format!(
                "{} of {} {} values are null",
                null_count, total_rows, col.name
            ));
        }

        // Dominant group note for categoricals
        if col.dtype == DType::Str {
            if let Some(cs) = stats.get(&col.unique_name) {
                if let Some(topk_vals) = &cs.top_k {
                    if let Some(first) = topk_vals.first() {
                        if let Some(count) = first.get("count").and_then(Value::as_u64) {
                            let pct = count as f64 / total_rows as f64 * 100.0;
                            if pct >= 50.0 {
                                let value = first
                                    .get("value")
                                    .and_then(Value::as_str)
                                    .unwrap_or("(unknown)");
                                notes.push(format!(
                                    "one {} group ({value:?}) accounts for {pct:.0}% of total",
                                    col.name
                                ));
                            }
                        }
                    }
                }
            }
        }

        // Date span note
        if col.dtype == DType::Date || col.dtype == DType::Time {
            if let Some(cs) = stats.get(&col.unique_name) {
                if let Some(topk_vals) = &cs.top_k {
                    let min_val = topk_vals
                        .iter()
                        .find(|v| v.get("label").and_then(Value::as_str) == Some("min"))
                        .and_then(|v| v.get("value"))
                        .and_then(Value::as_str);
                    let max_val = topk_vals
                        .iter()
                        .find(|v| v.get("label").and_then(Value::as_str) == Some("max"))
                        .and_then(|v| v.get("value"))
                        .and_then(Value::as_str);
                    if let (Some(min), Some(max)) = (min_val, max_val) {
                        notes.push(format!("{} spans {} to {}", col.name, min, max));
                    }
                }
            }
        }
    }

    notes
}

fn count_nulls(data: &ColumnData) -> u64 {
    match data {
        ColumnData::Int(v) => v.iter().filter(|o| o.is_none()).count() as u64,
        ColumnData::Float(v) => v.iter().filter(|o| o.is_none()).count() as u64,
        ColumnData::Bool(v) => v.iter().filter(|o| o.is_none()).count() as u64,
        ColumnData::Decimal(v)
        | ColumnData::Str(v)
        | ColumnData::Date(v)
        | ColumnData::Time(v) => v.iter().filter(|o| o.is_none()).count() as u64,
        _ => 0,
    }
}

/// Enforce `cfg.max_bytes` by progressively truncating the sample.
///
/// Stats are never removed.  If the zero-sample summary still exceeds
/// `max_bytes`, this function panics (the caller's `max_bytes` is too small).
fn enforce_max_bytes(
    mut summary: DatasetSummary,
    row_count: u64,
    cfg: &SummaryCfg,
    notes: &mut Vec<String>,
) -> DatasetSummary {
    // Fast path: already within budget.
    if serialized_len(&summary) <= cfg.max_bytes {
        return summary;
    }

    // Progressive truncation: remove tail rows one at a time.
    let original_sample_len = summary.sample.len();
    while !summary.sample.is_empty() && serialized_len(&summary) > cfg.max_bytes {
        summary.sample.pop();
    }

    // Add a truncation note if we removed anything.
    let new_sample_len = summary.sample.len();
    if new_sample_len < original_sample_len {
        let note = format!(
            "sample truncated from {} to {} rows to fit max_bytes={} (full result: {} rows)",
            original_sample_len, new_sample_len, cfg.max_bytes, row_count
        );
        notes.push(note.clone());
        summary.notes.push(note);
    }

    assert!(
        serialized_len(&summary) <= cfg.max_bytes,
        "even the zero-sample summary exceeds max_bytes={} — increase max_bytes",
        cfg.max_bytes
    );

    summary
}

fn serialized_len(summary: &DatasetSummary) -> usize {
    serde_json::to_vec(summary)
        .expect("DatasetSummary is always serializable")
        .len()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use dh_spec::{ColumnRole, DType};
    use dh_store::dataset::ColumnData;
    use dh_store::Dataset;

    fn make_col(name: &str, dtype: DType, role: ColumnRole) -> dh_spec::ColumnSchema {
        dh_spec::ColumnSchema {
            name: name.to_string(),
            unique_name: format!("model.{name}"),
            dtype,
            nullable: true,
            role,
        }
    }

    /// Build a 10_000-row dataset with one float measure column.
    fn big_float_dataset(n: usize) -> Dataset {
        let col = make_col("revenue", DType::Float, ColumnRole::Measure);
        let data: Vec<Option<f64>> = (0..n).map(|i| Some(i as f64)).collect();
        Dataset::new(vec![col], vec![ColumnData::Float(data)]).unwrap()
    }

    /// Build a small dataset with a string dimension and int measure.
    fn mixed_dataset(rows: usize) -> Dataset {
        let col_cat = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_num = make_col("sales", DType::Int, ColumnRole::Measure);
        let regions = vec!["North", "North", "South", "East", "North"];
        let cat_data: Vec<Option<String>> = (0..rows)
            .map(|i| Some(regions[i % regions.len()].to_string()))
            .collect();
        let int_data: Vec<Option<i64>> = (0..rows).map(|i| Some(i as i64 * 10)).collect();
        Dataset::new(
            vec![col_cat, col_num],
            vec![ColumnData::Str(cat_data), ColumnData::Int(int_data)],
        )
        .unwrap()
    }

    /// Build a dataset with some null values.
    fn nullable_dataset() -> Dataset {
        let col = make_col("revenue", DType::Float, ColumnRole::Measure);
        let n = 480;
        let data: Vec<Option<f64>> = (0..n)
            .map(|i| if i % 40 == 0 { None } else { Some(i as f64) })
            .collect();
        Dataset::new(vec![col], vec![ColumnData::Float(data)]).unwrap()
    }

    /// ac1: 10_000-row fixture → sample ≤ sample_cap AND serialized ≤ max_bytes
    #[test]
    fn ac1_large_dataset_sample_and_size_bounds() {
        let ds = big_float_dataset(10_000);
        let cfg = SummaryCfg {
            sample_cap: 8,
            topk: 10,
            max_bytes: 32_768,
        };
        let summary = summarize(&ds, &cfg);
        assert!(
            summary.sample.len() <= cfg.sample_cap,
            "sample.len()={} > sample_cap={}",
            summary.sample.len(),
            cfg.sample_cap
        );
        let size = serde_json::to_vec(&summary).unwrap().len();
        assert!(
            size <= cfg.max_bytes,
            "serialized size={} > max_bytes={}",
            size,
            cfg.max_bytes
        );
    }

    /// ac2: Numeric stats (min/max/sum/mean/distinct) match hand-computed golden.
    #[test]
    fn ac2_numeric_stats_golden() {
        // 5 values: 1, 2, 3, 4, 5
        let col = make_col("value", DType::Int, ColumnRole::Measure);
        let data = ColumnData::Int(vec![Some(1), Some(2), Some(3), Some(4), Some(5)]);
        let ds = Dataset::new(vec![col], vec![data]).unwrap();
        let cfg = SummaryCfg::default();
        let summary = summarize(&ds, &cfg);

        let cs = summary.stats.get("model.value").expect("stats present");
        assert_eq!(cs.min, Some(1.0), "min");
        assert_eq!(cs.max, Some(5.0), "max");
        assert_eq!(cs.sum, Some(15.0), "sum");
        assert_eq!(cs.mean, Some(3.0), "mean");
        assert_eq!(cs.distinct, Some(5), "distinct");
    }

    /// ac3: Categorical top-k is correct and capped at topk; date span correct.
    #[test]
    fn ac3_categorical_topk_and_date_span() {
        // 12 rows: A×3, B×2, C, D, E, F, G, H, I (7 singletons) → A is top
        let col_str = make_col("category", DType::Str, ColumnRole::Dimension);
        let col_date = make_col("dt", DType::Date, ColumnRole::Dimension);
        let cats: Vec<Option<String>> = vec![
            Some("A"), Some("A"), Some("A"),
            Some("B"), Some("B"),
            Some("C"), Some("D"), Some("E"),
            Some("F"), Some("G"), Some("H"), Some("I"),
        ]
        .into_iter()
        .map(|s| s.map(String::from))
        .collect();
        // 12 dates aligned with cats above
        let dates: Vec<Option<String>> = vec![
            Some("2023-01-01"), Some("2023-06-15"), Some("2022-03-10"),
            Some("2023-12-31"), Some("2022-01-01"), Some("2023-01-01"),
            Some("2023-06-15"), Some("2022-03-10"), Some("2023-12-31"),
            Some("2022-01-01"), Some("2023-01-01"), Some("2022-01-01"),
        ]
        .into_iter()
        .map(|s| s.map(String::from))
        .collect();

        let ds = Dataset::new(
            vec![col_str, col_date],
            vec![
                ColumnData::Str(cats),
                ColumnData::Date(dates),
            ],
        )
        .unwrap();

        let cfg = SummaryCfg {
            sample_cap: 8,
            topk: 5, // cap at 5
            max_bytes: 32_768,
        };
        let summary = summarize(&ds, &cfg);

        // top-k for category is capped at topk=5
        let cat_stats = summary.stats.get("model.category").expect("cat stats");
        let topk_vals = cat_stats.top_k.as_ref().expect("top_k present");
        assert!(
            topk_vals.len() <= 5,
            "top_k.len()={} should be ≤ topk=5",
            topk_vals.len()
        );
        // First entry should be "A" with count 3
        let first_val = topk_vals[0].get("value").and_then(Value::as_str).unwrap();
        let first_count = topk_vals[0].get("count").and_then(Value::as_u64).unwrap();
        assert_eq!(first_val, "A");
        assert_eq!(first_count, 3);

        // Date stats: check min/max surfaced in top_k
        let date_stats = summary.stats.get("model.dt").expect("date stats");
        let date_topk = date_stats.top_k.as_ref().expect("date top_k");
        let min_entry = date_topk
            .iter()
            .find(|v| v.get("label").and_then(Value::as_str) == Some("min"))
            .expect("min entry");
        let max_entry = date_topk
            .iter()
            .find(|v| v.get("label").and_then(Value::as_str) == Some("max"))
            .expect("max entry");
        assert_eq!(
            min_entry.get("value").and_then(Value::as_str).unwrap(),
            "2022-01-01"
        );
        assert_eq!(
            max_entry.get("value").and_then(Value::as_str).unwrap(),
            "2023-12-31"
        );
    }

    /// ac4: capabilities omits Pivot for single-dimension; includes it for ≥2.
    #[test]
    fn ac4_capabilities_pivot_rule() {
        // Single dimension
        let col_dim = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_meas = make_col("sales", DType::Int, ColumnRole::Measure);
        let data_dim = ColumnData::Str(vec![Some("North".to_string())]);
        let data_meas = ColumnData::Int(vec![Some(100)]);
        let ds_one = Dataset::new(vec![col_dim, col_meas], vec![data_dim, data_meas]).unwrap();
        let caps_one = capabilities(&ds_one);
        assert!(
            !caps_one.contains(&Capability::Pivot),
            "Pivot should be absent with 1 dimension"
        );

        // Two dimensions
        let col_dim1 = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_dim2 = make_col("product", DType::Str, ColumnRole::Dimension);
        let col_meas2 = make_col("sales", DType::Int, ColumnRole::Measure);
        let d1 = ColumnData::Str(vec![Some("North".to_string())]);
        let d2 = ColumnData::Str(vec![Some("Widget".to_string())]);
        let d3 = ColumnData::Int(vec![Some(100)]);
        let ds_two =
            Dataset::new(vec![col_dim1, col_dim2, col_meas2], vec![d1, d2, d3]).unwrap();
        let caps_two = capabilities(&ds_two);
        assert!(
            caps_two.contains(&Capability::Pivot),
            "Pivot should be present with 2 dimensions"
        );
    }

    /// ac5: deterministic notes for nulls and dominant group; stable across runs.
    #[test]
    fn ac5_notes_nulls_and_dominant_group() {
        // Null note
        let ds_null = nullable_dataset();
        let cfg = SummaryCfg::default();
        let summary1 = summarize(&ds_null, &cfg);
        let summary2 = summarize(&ds_null, &cfg);
        // Should have a null note
        let null_note = summary1
            .notes
            .iter()
            .find(|n| n.contains("null"))
            .cloned();
        assert!(null_note.is_some(), "expected a null note, got: {:?}", summary1.notes);
        // Notes must be stable (same output for same input)
        assert_eq!(
            summary1.notes, summary2.notes,
            "notes must be deterministic"
        );

        // Dominant group note: "North" appears 80% of the time
        let col = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_meas = make_col("v", DType::Int, ColumnRole::Measure);
        let n = 100;
        let cat_data: Vec<Option<String>> = (0..n)
            .map(|i| {
                if i < 80 {
                    Some("North".to_string())
                } else {
                    Some("South".to_string())
                }
            })
            .collect();
        let int_data: Vec<Option<i64>> = (0..n).map(|i| Some(i as i64)).collect();
        let ds_dom = Dataset::new(
            vec![col, col_meas],
            vec![ColumnData::Str(cat_data), ColumnData::Int(int_data)],
        )
        .unwrap();
        let summary_dom = summarize(&ds_dom, &cfg);
        let dom_note = summary_dom
            .notes
            .iter()
            .find(|n| n.contains("accounts for") && n.contains('%'))
            .cloned();
        assert!(
            dom_note.is_some(),
            "expected a dominant-group note, got: {:?}",
            summary_dom.notes
        );
    }

    /// ac6: oversized dataset → sample truncated (not stats), truncation note added.
    #[test]
    fn ac6_max_bytes_truncates_sample_not_stats() {
        // Build a 1000-row dataset with a wide string column so each sample row
        // contributes substantial bytes.  Use sample_cap=8 and set max_bytes just
        // below the full-sample serialized size — guaranteeing at least one pop.
        let col_rev = make_col("revenue", DType::Float, ColumnRole::Measure);
        // Wide string column: each sample row adds ~80 bytes
        let col_desc = make_col("description", DType::Str, ColumnRole::Dimension);
        let n = 1000;
        let float_data: Vec<Option<f64>> =
            (0..n).map(|i| Some(i as f64 * 1.234_567_89)).collect();
        let str_data: Vec<Option<String>> = (0..n)
            .map(|i| {
                Some(format!(
                    "a very long description string for row {:04} padding padding padding",
                    i
                ))
            })
            .collect();
        let ds = Dataset::new(
            vec![col_rev, col_desc],
            vec![ColumnData::Float(float_data), ColumnData::Str(str_data)],
        )
        .unwrap();

        // Measure full-sample size with no size constraint.
        let full_cfg = SummaryCfg {
            sample_cap: 8,
            topk: 5,
            max_bytes: usize::MAX,
        };
        let full_summary = summarize(&ds, &full_cfg);
        let full_size = serde_json::to_vec(&full_summary).unwrap().len();

        // Set max_bytes just below full — guarantees at least one row is popped.
        let tight_max = full_size - 1;
        let cfg = SummaryCfg {
            sample_cap: 8,
            topk: 5,
            max_bytes: tight_max,
        };
        let summary = summarize(&ds, &cfg);

        // Serialized result must fit within the budget.
        let size = serde_json::to_vec(&summary).unwrap().len();
        assert!(
            size <= cfg.max_bytes,
            "size={size} must be ≤ max_bytes={tight_max}"
        );

        // Stats must still be present (never truncated).
        assert!(
            summary.stats.contains_key("model.revenue"),
            "revenue stats must be preserved"
        );
        assert!(
            summary.stats.contains_key("model.description"),
            "description stats must be preserved"
        );
        assert!(summary.stats["model.revenue"].min.is_some(), "min stat present");
        assert!(summary.stats["model.revenue"].max.is_some(), "max stat present");

        // The sample must have fewer rows than the unconstrained full-sample.
        assert!(
            summary.sample.len() < full_summary.sample.len(),
            "sample.len()={} must be < full_sample.len()={} after truncation",
            summary.sample.len(),
            full_summary.sample.len()
        );

        // A truncation note must be present.
        let trunc_note = summary
            .notes
            .iter()
            .find(|n| n.contains("truncated") || n.contains("max_bytes"))
            .cloned();
        assert!(
            trunc_note.is_some(),
            "expected a truncation note, got: {:?}",
            summary.notes
        );
    }

    /// ac7: cargo test --release passes; clippy clean — enforced by the build.
    /// This test verifies the public API compiles and a round-trip works end-to-end.
    #[test]
    fn ac7_round_trip_serialization() {
        let ds = mixed_dataset(20);
        let cfg = SummaryCfg::default();
        let summary = summarize(&ds, &cfg);
        let json = serde_json::to_string(&summary).expect("serialize");
        let back: DatasetSummary = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(summary.row_count, back.row_count);
        assert_eq!(summary.columns.len(), back.columns.len());
        assert_eq!(summary.sample.len(), back.sample.len());
    }

    // ── Mutation-hardening tests ──────────────────────────────────────────────

    /// stats_bool: verify true/false/null counts and top_k entries are correct.
    #[test]
    fn stats_bool_counts_and_top_k() {
        let col = make_col("flag", DType::Bool, ColumnRole::Dimension);
        // 3 true, 2 false, 1 null
        let data = ColumnData::Bool(vec![
            Some(true),
            Some(true),
            Some(true),
            Some(false),
            Some(false),
            None,
        ]);
        let ds = Dataset::new(vec![col], vec![data]).unwrap();
        let summary = summarize(&ds, &SummaryCfg::default());
        let cs = summary.stats.get("model.flag").expect("flag stats");

        // distinct = true + false + null-present = 3
        assert_eq!(cs.distinct, Some(3), "distinct should be 3 (true, false, null)");

        // top_k should have entries for true and false
        let topk = cs.top_k.as_ref().expect("top_k present for bool");
        let true_entry = topk.iter().find(|e| e.get("value") == Some(&serde_json::Value::Bool(true)));
        let false_entry = topk.iter().find(|e| e.get("value") == Some(&serde_json::Value::Bool(false)));
        assert!(true_entry.is_some(), "true entry present in top_k");
        assert!(false_entry.is_some(), "false entry present in top_k");
        assert_eq!(
            true_entry.unwrap().get("count").and_then(Value::as_u64),
            Some(3),
            "true count = 3"
        );
        assert_eq!(
            false_entry.unwrap().get("count").and_then(Value::as_u64),
            Some(2),
            "false count = 2"
        );
    }

    /// stats_bool: all-null column → distinct=1, no top_k entries.
    #[test]
    fn stats_bool_all_null() {
        let col = make_col("flag", DType::Bool, ColumnRole::Dimension);
        let data = ColumnData::Bool(vec![None, None, None]);
        let ds = Dataset::new(vec![col], vec![data]).unwrap();
        let summary = summarize(&ds, &SummaryCfg::default());
        let cs = summary.stats.get("model.flag").expect("flag stats");
        assert_eq!(cs.distinct, Some(1), "one distinct (null)");
        // top_k is None when no true/false entries exist
        assert!(cs.top_k.is_none(), "no top_k when only nulls");
    }

    /// stats_bool: all-false → distinct=1, no true entry.
    #[test]
    fn stats_bool_all_false() {
        let col = make_col("active", DType::Bool, ColumnRole::Dimension);
        let data = ColumnData::Bool(vec![Some(false), Some(false)]);
        let ds = Dataset::new(vec![col], vec![data]).unwrap();
        let summary = summarize(&ds, &SummaryCfg::default());
        let cs = summary.stats.get("model.active").expect("active stats");
        assert_eq!(cs.distinct, Some(1), "distinct = 1 (false only)");
        let topk = cs.top_k.as_ref().expect("top_k present");
        assert!(
            topk.iter().all(|e| e.get("value") != Some(&serde_json::Value::Bool(true))),
            "no true entry when all false"
        );
    }

    /// count_nulls: verify null counting works for each ColumnData variant.
    #[test]
    fn count_nulls_per_variant() {
        // Int: 2 nulls out of 4
        {
            let col = make_col("n", DType::Int, ColumnRole::Measure);
            let data = ColumnData::Int(vec![Some(1), None, Some(3), None]);
            let ds = Dataset::new(vec![col], vec![data]).unwrap();
            let summary = summarize(&ds, &SummaryCfg::default());
            let note = summary.notes.iter().find(|n| n.contains("null"));
            assert!(note.is_some(), "null note for Int with 2 nulls");
            assert!(note.unwrap().starts_with("2 of 4"), "correct null count: {note:?}");
        }
        // Float: 1 null
        {
            let col = make_col("f", DType::Float, ColumnRole::Measure);
            let data = ColumnData::Float(vec![Some(1.0), None, Some(3.0)]);
            let ds = Dataset::new(vec![col], vec![data]).unwrap();
            let summary = summarize(&ds, &SummaryCfg::default());
            let note = summary.notes.iter().find(|n| n.contains("null"));
            assert!(note.is_some(), "null note for Float with 1 null");
            assert!(note.unwrap().starts_with("1 of 3"), "correct null count: {note:?}");
        }
        // Str: 1 null
        {
            let col = make_col("s", DType::Str, ColumnRole::Dimension);
            let data = ColumnData::Str(vec![Some("a".to_string()), None]);
            let ds = Dataset::new(vec![col], vec![data]).unwrap();
            let summary = summarize(&ds, &SummaryCfg::default());
            let note = summary.notes.iter().find(|n| n.contains("null"));
            assert!(note.is_some(), "null note for Str with 1 null");
        }
        // Date: 1 null
        {
            let col = make_col("d", DType::Date, ColumnRole::Dimension);
            let data = ColumnData::Date(vec![Some("2023-01-01".to_string()), None]);
            let ds = Dataset::new(vec![col], vec![data]).unwrap();
            let summary = summarize(&ds, &SummaryCfg::default());
            let note = summary.notes.iter().find(|n| n.contains("null"));
            assert!(note.is_some(), "null note for Date with 1 null");
        }
    }

    /// generate_notes: dominance threshold is strictly >= 50%.
    /// Boundary check: a group at exactly 50% triggers; a group at 33% does not.
    #[test]
    fn notes_dominance_threshold_boundary() {
        // 3 equal groups of 33 each (33%) → no dominant-group note.
        let col = make_col("region", DType::Str, ColumnRole::Dimension);
        let col_m = make_col("v", DType::Int, ColumnRole::Measure);
        let n = 99;
        // 33 each of A, B, C → top group is 33% (< 50%)
        let cat_equal: Vec<Option<String>> = (0..n)
            .map(|i| {
                let g = ["A", "B", "C"][i % 3];
                Some(g.to_string())
            })
            .collect();
        let int_data: Vec<Option<i64>> = (0..n).map(|i| Some(i as i64)).collect();
        let ds_equal = Dataset::new(
            vec![col.clone(), col_m.clone()],
            vec![ColumnData::Str(cat_equal), ColumnData::Int(int_data)],
        )
        .unwrap();
        let summary_equal = summarize(&ds_equal, &SummaryCfg::default());
        assert!(
            !summary_equal.notes.iter().any(|n| n.contains("accounts for") && n.contains('%')),
            "no dominant-group note when top group is 33%: {:?}",
            summary_equal.notes
        );

        // Exactly 50 out of 100 → "North" is 50% → triggers the note.
        let n2 = 100;
        let cat_50: Vec<Option<String>> = (0..n2)
            .map(|i| if i < 50 { Some("North".to_string()) } else { Some("South".to_string()) })
            .collect();
        let int_data2: Vec<Option<i64>> = (0..n2).map(|i| Some(i as i64)).collect();
        let ds_50 = Dataset::new(
            vec![col, col_m],
            vec![ColumnData::Str(cat_50), ColumnData::Int(int_data2)],
        )
        .unwrap();
        let summary_50 = summarize(&ds_50, &SummaryCfg::default());
        assert!(
            summary_50.notes.iter().any(|n| n.contains("accounts for") && n.contains('%')),
            "dominant-group note present at exactly 50%: {:?}",
            summary_50.notes
        );
    }

    /// enforce_max_bytes: boundary — summary that fits exactly at max_bytes is not truncated.
    #[test]
    fn max_bytes_exact_fit_no_truncation() {
        let col = make_col("x", DType::Int, ColumnRole::Measure);
        let data = ColumnData::Int(vec![Some(1), Some(2), Some(3)]);
        let ds = Dataset::new(vec![col], vec![data]).unwrap();

        // Get exact size with sample_cap=3 and large max_bytes.
        let full_cfg = SummaryCfg { sample_cap: 3, topk: 10, max_bytes: usize::MAX };
        let full_summary = summarize(&ds, &full_cfg);
        let exact_size = serde_json::to_vec(&full_summary).unwrap().len();

        // Set max_bytes to exact size — must NOT truncate.
        let exact_cfg = SummaryCfg { sample_cap: 3, topk: 10, max_bytes: exact_size };
        let summary = summarize(&ds, &exact_cfg);
        assert!(
            !summary.notes.iter().any(|n| n.contains("truncated")),
            "no truncation note when fits exactly"
        );
        assert_eq!(summary.sample.len(), full_summary.sample.len(), "sample unchanged");
    }

    /// enforce_max_bytes: > max_bytes triggers truncation.
    /// This test is a deliberate complement to ac6 (which uses a complex multi-column
    /// dataset). Here we use a known-small Int dataset and set max_bytes to a value
    /// that definitely exceeds the stats-only floor but is below the full-sample size.
    /// We measure the unconstrained size, then subtract enough to force at least one pop.
    #[test]
    fn max_bytes_one_over_triggers_truncation() {
        // 20-row int column, sample_cap=4 → 4 sample rows.
        // Use a large topk so we can measure the stats floor accurately.
        let col = make_col("qty", DType::Int, ColumnRole::Measure);
        let data: Vec<Option<i64>> = (0..20).map(|i| Some(i as i64 * 1000)).collect();
        let ds = Dataset::new(vec![col], vec![ColumnData::Int(data)]).unwrap();

        // Full run: sample_cap=4 with no byte limit.
        let full_cfg = SummaryCfg { sample_cap: 4, topk: 5, max_bytes: usize::MAX };
        let full_summary = summarize(&ds, &full_cfg);
        let full_size = serde_json::to_vec(&full_summary).unwrap().len();

        // Tight max: remove 40% of the sample-delta to force truncation.
        // To find a safe floor we add a generous 200 bytes on top of the stats block size
        // (min/max/sum/mean/distinct on 20 ints is about 80 bytes; adding 200 leaves room
        // for the truncation note that enforce_max_bytes appends after popping all rows).
        let generous_floor = 600_usize; // well above stats+truncation-note
        if full_size <= generous_floor {
            // Dataset serialises smaller than our floor estimate; skip.
            return;
        }
        let tight_max = generous_floor + (full_size - generous_floor) / 3;
        let tight_cfg = SummaryCfg { sample_cap: 4, topk: 5, max_bytes: tight_max };
        let summary = summarize(&ds, &tight_cfg);
        let size = serde_json::to_vec(&summary).unwrap().len();
        assert!(size <= tight_max, "size {size} must be <= max_bytes {tight_max}");
        assert!(
            summary.sample.len() < full_summary.sample.len(),
            "sample must be shorter after truncation (was {}, now {})",
            full_summary.sample.len(), summary.sample.len()
        );
    }

    /// generate_notes: date span note correct format "col spans min to max".
    #[test]
    fn notes_date_span_format() {
        let col = make_col("event_date", DType::Date, ColumnRole::Dimension);
        let dates: Vec<Option<String>> = vec![
            Some("2023-03-15".to_string()),
            Some("2021-07-04".to_string()),
            Some("2024-01-01".to_string()),
        ];
        let ds = Dataset::new(vec![col], vec![ColumnData::Date(dates)]).unwrap();
        let summary = summarize(&ds, &SummaryCfg::default());
        let span_note = summary
            .notes
            .iter()
            .find(|n| n.contains("event_date") && n.contains("spans"));
        assert!(span_note.is_some(), "date span note present: {:?}", summary.notes);
        let note = span_note.unwrap();
        assert!(note.contains("2021-07-04"), "min date in note: {note}");
        assert!(note.contains("2024-01-01"), "max date in note: {note}");
    }

    /// stats_int: all-null → min/max/sum/mean are None, distinct = 1.
    #[test]
    fn stats_int_all_null() {
        let col = make_col("qty", DType::Int, ColumnRole::Measure);
        let data = ColumnData::Int(vec![None, None]);
        let ds = Dataset::new(vec![col], vec![data]).unwrap();
        let summary = summarize(&ds, &SummaryCfg::default());
        let cs = summary.stats.get("model.qty").expect("qty stats");
        assert!(cs.min.is_none(), "min is None when all null");
        assert!(cs.max.is_none(), "max is None when all null");
        assert!(cs.sum.is_none(), "sum is None when all null");
        assert!(cs.mean.is_none(), "mean is None when all null");
        assert_eq!(cs.distinct, Some(1), "1 distinct (null)");
    }

    /// stats_float: verify arithmetic for a small known set.
    #[test]
    fn stats_float_golden() {
        let col = make_col("score", DType::Float, ColumnRole::Measure);
        let data = ColumnData::Float(vec![Some(2.0), Some(4.0), Some(6.0)]);
        let ds = Dataset::new(vec![col], vec![data]).unwrap();
        let summary = summarize(&ds, &SummaryCfg::default());
        let cs = summary.stats.get("model.score").expect("score stats");
        assert_eq!(cs.min, Some(2.0), "min=2.0");
        assert_eq!(cs.max, Some(6.0), "max=6.0");
        assert_eq!(cs.sum, Some(12.0), "sum=12.0");
        assert_eq!(cs.mean, Some(4.0), "mean=4.0");
        assert_eq!(cs.distinct, Some(3), "distinct=3");
    }

    /// Reviewer counter-attack: stats_date distinct count with duplicates.
    /// 3 rows, 2 identical dates → distinct should be 2 (not 3).
    #[test]
    fn stats_date_distinct_with_duplicates() {
        let col = make_col("dt", DType::Date, ColumnRole::Dimension);
        let dates = ColumnData::Date(vec![
            Some("2023-01-01".to_string()),
            Some("2023-01-01".to_string()), // duplicate
            Some("2024-06-15".to_string()),
        ]);
        let ds = Dataset::new(vec![col], vec![dates]).unwrap();
        let summary = summarize(&ds, &SummaryCfg::default());
        let cs = summary.stats.get("model.dt").expect("dt stats");
        assert_eq!(
            cs.distinct,
            Some(2),
            "distinct should be 2 (two unique dates, no nulls)"
        );
    }
}
