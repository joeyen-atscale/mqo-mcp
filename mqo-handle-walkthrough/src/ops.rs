//! Handle-op kernels for the mem-store backend.
//!
//! Each function takes a slice of JSON rows and returns a new Vec of rows
//! (immutable-derive semantics — input is never mutated).

use serde_json::Value;

/// Infer a simple schema from the first row's keys.
pub fn infer_schema(rows: &[Value]) -> Vec<mqo_duckdb_handle_store::ColumnSchema> {
    let Some(first) = rows.first() else {
        return vec![];
    };
    let obj = match first.as_object() {
        Some(o) => o,
        None => return vec![],
    };
    obj.keys()
        .map(|k| mqo_duckdb_handle_store::ColumnSchema {
            name: k.clone(),
            ty: "string".to_string(),
        })
        .collect()
}

/// `period_over_period`: add a `yoy_change` column.
///
/// For each row with `year == current_year`, find a matching row with
/// `year == current_year - 1` (same state + month suffix) and compute
/// `web_sales - prior_web_sales`.  Rows for prior years pass through
/// unchanged (yoy_change = null).
pub fn period_over_period(rows: &[Value]) -> Vec<Value> {
    // Determine the max year in the dataset.
    let current_year = rows
        .iter()
        .filter_map(|r| r.get("year").and_then(|v| v.as_f64()))
        .fold(f64::NEG_INFINITY, f64::max) as i64;

    rows.iter()
        .map(|row| {
            let mut obj = row.as_object().cloned().unwrap_or_default();
            let year = obj.get("year").and_then(|v| v.as_f64()).unwrap_or(0.0) as i64;
            if year == current_year {
                let state = obj
                    .get("state")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let month = obj
                    .get("month")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                // Derive prior-year month label: replace year portion.
                let prior_year = current_year - 1;
                let prior_month = month.replacen(&current_year.to_string(), &prior_year.to_string(), 1);
                let current_sales = obj
                    .get("web_sales")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                // Find matching prior-year row.
                let prior_sales = rows.iter().find(|r| {
                    r.get("state").and_then(|v| v.as_str()) == Some(&state)
                        && r.get("month").and_then(|v| v.as_str()) == Some(&prior_month)
                        && r.get("year").and_then(|v| v.as_f64()).map(|y| y as i64)
                            == Some(prior_year)
                });
                let yoy = prior_sales
                    .and_then(|r| r.get("web_sales"))
                    .and_then(|v| v.as_f64())
                    .map(|p| current_sales - p);
                obj.insert(
                    "yoy_change".to_string(),
                    match yoy {
                        Some(v) => Value::from(v),
                        None => Value::Null,
                    },
                );
            } else {
                obj.insert("yoy_change".to_string(), Value::Null);
            }
            Value::Object(obj)
        })
        .collect()
}

/// `slice`: filter rows to those where `state == target_state`.
pub fn slice_by_state(rows: &[Value], target_state: &str) -> Vec<Value> {
    rows.iter()
        .filter(|r| r.get("state").and_then(|v| v.as_str()) == Some(target_state))
        .cloned()
        .collect()
}

/// `chart`: produce a Vega-Lite line spec from the rows.
///
/// x-axis: `month` field; y-axis: `web_sales` field; colour: `state`.
pub fn chart_vega_lite(rows: &[Value], title: &str) -> Value {
    // Embed rows as inline data.
    serde_json::json!({
        "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
        "title": title,
        "mark": "line",
        "data": { "values": rows },
        "encoding": {
            "x": { "field": "month", "type": "temporal", "title": "Month" },
            "y": { "field": "web_sales", "type": "quantitative", "title": "Web Sales" },
            "color": { "field": "state", "type": "nominal" }
        }
    })
}
