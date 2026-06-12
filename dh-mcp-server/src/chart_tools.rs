//! Handle-aware BI/chart tools for `dh-mcp-server`.
//!
//! Bridges the dataset-handle store → chart crate pipeline:
//!
//! 1. Read the [`Dataset`] from the handle store (rows stay server-side).
//! 2. Convert the columnar Dataset into the `{rows, bound}` JSON shape that the
//!    chart crates expect.
//! 3. Feed the payload to the profiler → recommender → emitter / bi-asset-bundle
//!    chain (reusing the shipped chart crates verbatim, with no new chart logic).
//! 4. Return **only** the spec / asset bundle to the caller — never the rows.
//!
//! All functions are deterministic, side-effect-free, and carry `readOnlyHint: true`.

use dh_spec::{ColumnRole, DatasetHandle};
use dh_store::Store;
use serde_json::{json, Value};

// ── Public tool constants ──────────────────────────────────────────────────────

/// Maximum rows fed to chart crates from a stored handle.
///
/// Chart specs embed row data inline.  Keeping this well below context-window
/// limits ensures the spec stays compact while still providing enough fidelity
/// for a useful visualisation.  The K=500 bound mirrors
/// `mqo-mcp-server`'s `BUILD_BI_ASSET_MAX_ROWS`.
pub const CHART_MAX_ROWS: usize = 500;

// ── Dataset → `{rows, bound}` conversion ──────────────────────────────────────

/// Convert a stored [`dh_store::Dataset`] into the `{rows, bound}` JSON envelope
/// consumed by the `mqo-result-profiler` / `mqo-bi-asset-bundle` chain.
///
/// The `rows` array contains at most [`CHART_MAX_ROWS`] rows.  The `bound` object
/// lists the column names partitioned by role so the profiler can derive types.
///
/// Returns `Err(String)` when the handle is not found, expired, or empty.
fn dataset_to_payload(store: &Store, handle: &DatasetHandle) -> Result<Value, String> {
    let ds = store
        .get(handle)
        .map_err(|e| format!("handle lookup failed: {e}"))?;

    if ds.row_count() == 0 {
        return Err("dataset is empty — cannot build a chart from zero rows".to_string());
    }

    // Cap rows delivered to the chart crates.
    let cap = ds.row_count().min(CHART_MAX_ROWS);

    // Build row-oriented JSON objects from the columnar store, capped at `cap`.
    let mut rows: Vec<Value> = Vec::with_capacity(cap);
    for row_idx in 0..cap {
        let mut obj = serde_json::Map::new();
        for (col, col_data) in ds.columns.iter().zip(ds.data.iter()) {
            let v = extract_json_value(col_data, row_idx);
            obj.insert(col.name.clone(), v);
        }
        rows.push(Value::Object(obj));
    }

    // Build the `bound` object: lists of column names by role.
    let mut measures: Vec<Value> = Vec::new();
    let mut dimensions: Vec<Value> = Vec::new();
    for col in &ds.columns {
        match col.role {
            ColumnRole::Measure | ColumnRole::Derived => {
                measures.push(Value::String(col.name.clone()));
            }
            ColumnRole::Dimension => {
                dimensions.push(Value::String(col.name.clone()));
            }
        }
    }

    // Build a catalog JSON for the profiler so it can label columns by name.
    // We derive labels directly from the column schema (name → label).
    let catalog_columns: Vec<Value> = ds
        .columns
        .iter()
        .map(|col| {
            let kind = match col.role {
                ColumnRole::Measure | ColumnRole::Derived => "measure",
                ColumnRole::Dimension => "dimension",
            };
            json!({
                "unique_name": col.name,
                "label": col.name,
                "kind": kind
            })
        })
        .collect();

    Ok(json!({
        "rows": rows,
        "bound": {
            "measures": measures,
            "dimensions": dimensions
        },
        "_catalog": catalog_columns
    }))
}

/// Extract a JSON [`Value`] from a [`dh_store::ColumnData`] at `row_idx`.
fn extract_json_value(col_data: &dh_store::ColumnData, row_idx: usize) -> Value {
    use dh_store::ColumnData;
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

// ── Tool: dataset_chart ────────────────────────────────────────────────────────

/// Handle the `dataset_chart` MCP tool.
///
/// Reads the handle's data from the store, builds a row-oriented payload,
/// and emits a Vega-Lite v5 spec using the explicit `chart_type`, `x_col`,
/// and `y_cols` parameters.
///
/// No new handle is created.  No rows are returned to the caller.
#[must_use]
pub fn handle_dataset_chart(store: &Store, args: &Value) -> Value {
    let handle = match parse_handle(args) {
        Ok(h) => h,
        Err(v) => return v,
    };

    let chart_type = args
        .get("chart_type")
        .and_then(Value::as_str)
        .unwrap_or("bar");

    let Some(x_col) = args.get("x_col").and_then(Value::as_str) else {
        return chart_err("bad_param", "missing required field 'x_col'");
    };
    let x_col = x_col.to_string();

    let y_cols: Vec<String> = args
        .get("y_cols")
        .and_then(Value::as_array)
        .map_or(vec![], |a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        });
    if y_cols.is_empty() {
        return chart_err("bad_param", "y_cols must be a non-empty array");
    }

    let title = args
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let vl_mark = match chart_type {
        "bar" | "line" | "area" | "point" => chart_type,
        other => {
            return chart_err(
                "bad_param",
                &format!("unsupported chart_type '{other}'; use bar|line|area|point"),
            )
        }
    };

    // Load the dataset and convert to row-oriented JSON (capped).
    let payload = match dataset_to_payload(store, &handle) {
        Ok(p) => p,
        Err(e) => return chart_err("handle_error", &e),
    };

    let rows: Vec<Value> = payload
        .get("rows")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // Verify x_col and y_cols are present in the first row (best-effort).
    if !rows.is_empty() {
        let first = &rows[0];
        if first.get(&x_col).is_none() {
            return chart_err(
                "unknown_column",
                &format!("x_col '{x_col}' not found in dataset"),
            );
        }
        for y in &y_cols {
            if first.get(y).is_none() {
                return chart_err(
                    "unknown_column",
                    &format!("y_col '{y}' not found in dataset"),
                );
            }
        }
    }

    let spec = build_explicit_spec(vl_mark, &x_col, &y_cols, &title, &rows);
    chart_ok(&spec)
}

/// Build a Vega-Lite v5 spec with explicit x/y column bindings.
fn build_explicit_spec(
    mark: &str,
    x_col: &str,
    y_cols: &[String],
    title: &str,
    rows: &[Value],
) -> Value {
    let mut spec = if y_cols.len() == 1 {
        json!({
            "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
            "mark": mark,
            "data": { "values": rows },
            "encoding": {
                "x": { "field": x_col, "type": "nominal" },
                "y": { "field": y_cols[0], "type": "quantitative", "aggregate": "sum" }
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
                        "y": { "field": y, "type": "quantitative", "aggregate": "sum" }
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

// ── Tool: build_bi_asset ───────────────────────────────────────────────────────

/// Handle the `build_bi_asset` MCP tool.
///
/// Reads the handle's data from the store, derives a synthetic catalog from the
/// column schema, and invokes `mqo-bi-asset-bundle::build_asset` to produce a
/// complete `bi-asset.v1` bundle (`{title, description, caveats, vega_spec,
/// profile_summary}`).
///
/// No rows are returned.  The Vega-Lite spec embedded in the bundle contains
/// the data; that is the expected contract for this tool.
#[must_use]
pub fn handle_build_bi_asset(store: &Store, args: &Value) -> Value {
    let handle = match parse_handle(args) {
        Ok(h) => h,
        Err(v) => return v,
    };

    let payload = match dataset_to_payload(store, &handle) {
        Ok(p) => p,
        Err(e) => return chart_err("handle_error", &e),
    };

    // Split out the catalog we derived from the column schema.
    let catalog = json!({ "columns": payload["_catalog"] });

    // Strip the _catalog field before passing to build_asset (it only needs rows+bound).
    let response = json!({
        "rows": payload["rows"],
        "bound": payload["bound"]
    });

    let row_count = response["rows"]
        .as_array()
        .map_or(0, Vec::len);

    if row_count == 0 {
        return chart_err("empty_rows", "dataset is empty — cannot build a BI asset");
    }

    match mqo_bi_asset_bundle::build_asset(&response, &catalog) {
        Ok(asset) => {
            let asset_val = serde_json::to_value(&asset).unwrap_or_else(|e| {
                json!({ "error": e.to_string() })
            });
            chart_ok(&asset_val)
        }
        Err(e) => chart_err("build_asset_error", &e.to_string()),
    }
}

// ── Tool: compose_dashboard ────────────────────────────────────────────────────

/// Handle the `compose_dashboard` MCP tool (P1 — multi-panel VL5 concat spec).
///
/// Accepts a `handles` array (each element is a `DatasetHandle`) plus an optional
/// `layout` (`"grid"` | `"vertical"` | `"horizontal"`) and `columns` integer.
/// Builds a BI asset for each handle, then composes them into a multi-panel
/// Vega-Lite v5 `concat` spec via `mqo-dashboard-composer`.
///
/// No rows are returned.
#[must_use]
pub fn handle_compose_dashboard(store: &Store, args: &Value) -> Value {
    let Some(title) = args.get("title").and_then(Value::as_str) else {
        return chart_err("bad_param", "compose_dashboard requires a 'title' string");
    };

    let Some(handles_val) = args.get("handles").and_then(Value::as_array) else {
        return chart_err(
            "bad_param",
            "compose_dashboard requires a 'handles' array of DatasetHandle objects",
        );
    };

    if handles_val.is_empty() {
        return chart_err("no_panels", "compose_dashboard requires at least one handle");
    }

    if handles_val.len() > 20 {
        return chart_err(
            "input_too_large",
            &format!(
                "compose_dashboard accepts at most 20 handles; got {}",
                handles_val.len()
            ),
        );
    }

    // Parse layout.
    let layout = match args.get("layout").and_then(Value::as_str).unwrap_or("grid") {
        "vertical" => mqo_dashboard_composer::Layout::Vertical,
        "horizontal" => mqo_dashboard_composer::Layout::Horizontal,
        _ => mqo_dashboard_composer::Layout::Grid,
    };

    // Parse columns.
    let columns: u32 = args
        .get("columns")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(2)
        .max(1);

    // Build a BI asset for each handle, collecting bundles.
    let mut bundles: Vec<mqo_dashboard_composer::BiAssetBundle> = Vec::new();

    for (i, handle_val) in handles_val.iter().enumerate() {
        let handle: DatasetHandle = match serde_json::from_value(handle_val.clone()) {
            Ok(h) => h,
            Err(e) => {
                return chart_err(
                    "bad_param",
                    &format!("handles[{i}] is not a valid DatasetHandle: {e}"),
                )
            }
        };

        let payload = match dataset_to_payload(store, &handle) {
            Ok(p) => p,
            Err(e) => {
                return chart_err(
                    "handle_error",
                    &format!("handles[{i}]: {e}"),
                )
            }
        };

        let catalog = json!({ "columns": payload["_catalog"] });
        let response = json!({ "rows": payload["rows"], "bound": payload["bound"] });

        let asset = match mqo_bi_asset_bundle::build_asset(&response, &catalog) {
            Ok(a) => a,
            Err(e) => {
                return chart_err(
                    "build_asset_error",
                    &format!("handles[{i}]: {e}"),
                )
            }
        };

        let bundle_val = serde_json::to_value(&asset).unwrap_or_else(|e| {
            json!({ "error": e.to_string() })
        });

        match serde_json::from_value::<mqo_dashboard_composer::BiAssetBundle>(bundle_val) {
            Ok(b) => bundles.push(b),
            Err(e) => {
                return chart_err(
                    "internal_error",
                    &format!("handles[{i}] asset could not be parsed as BiAssetBundle: {e}"),
                )
            }
        }
    }

    let dashboard = mqo_dashboard_composer::build_dashboard(&bundles, title, layout, columns);
    let dashboard_val = serde_json::to_value(&dashboard).unwrap_or_else(|e| {
        json!({ "error": e.to_string() })
    });
    chart_ok(&dashboard_val)
}

// ── Argument parsing helpers ───────────────────────────────────────────────────

fn parse_handle(args: &Value) -> Result<DatasetHandle, Value> {
    let h = args
        .get("handle")
        .ok_or_else(|| chart_err("bad_param", "missing 'handle'"))?;
    serde_json::from_value(h.clone())
        .map_err(|e| chart_err("bad_param", &format!("invalid handle: {e}")))
}

// ── Result envelope helpers ────────────────────────────────────────────────────

pub(crate) fn chart_ok(payload: &Value) -> Value {
    let text = serde_json::to_string(payload).unwrap_or_default();
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": payload,
        "isError": false
    })
}

pub(crate) fn chart_err(code: &str, detail: &str) -> Value {
    let payload = json!({ "error": { "code": code, "detail": detail } });
    let text = serde_json::to_string(&payload).unwrap_or_default();
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": payload,
        "isError": true
    })
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use dh_spec::{ColumnRole, ColumnSchema, DType};
    use dh_store::{ColumnData, Dataset, Store};
    use serde_json::json;

    fn make_schema(name: &str, role: ColumnRole, dtype: DType) -> ColumnSchema {
        ColumnSchema {
            name: name.to_string(),
            unique_name: format!("model.{name}"),
            dtype,
            nullable: false,
            role,
        }
    }

    fn make_test_dataset() -> Dataset {
        let col_year = make_schema("year", ColumnRole::Dimension, DType::Str);
        let col_revenue = make_schema("revenue", ColumnRole::Measure, DType::Float);
        Dataset::new(
            vec![col_year, col_revenue],
            vec![
                ColumnData::Str(vec![
                    Some("2021".to_string()),
                    Some("2022".to_string()),
                    Some("2023".to_string()),
                ]),
                ColumnData::Float(vec![Some(100.0), Some(200.0), Some(150.0)]),
            ],
        )
        .expect("valid dataset")
    }

    fn store_with_dataset() -> (Store, dh_spec::DatasetHandle) {
        let store = Store::new(0);
        let ds = make_test_dataset();
        let handle = store.put(ds, 3600);
        (store, handle)
    }

    // ── AC-1: dataset_chart returns a VL5 spec with no rows in the response ──

    #[test]
    fn ac1_dataset_chart_returns_vl5_spec_no_rows_in_response() {
        let (store, handle) = store_with_dataset();
        let args = json!({
            "handle": handle,
            "chart_type": "bar",
            "x_col": "year",
            "y_cols": ["revenue"],
            "title": "Revenue by Year"
        });
        let result = handle_dataset_chart(&store, &args);

        // Tool returned success
        assert_eq!(result["isError"], false, "expected isError=false: {result:?}");

        // Response contains a spec, not a rows array
        let spec = &result["structuredContent"];
        assert!(spec.is_object(), "structuredContent should be an object");
        assert!(spec.get("rows").is_none(), "rows must NOT appear in the response");
        assert!(
            spec.get("$schema").is_some() || spec.get("mark").is_some() || spec.get("data").is_some(),
            "expected a Vega-Lite spec: {spec:?}"
        );
    }

    // ── AC-2: dataset_chart returns a Vega-Lite v5 $schema ───────────────────

    #[test]
    fn ac2_dataset_chart_vl5_schema_url() {
        let (store, handle) = store_with_dataset();
        let args = json!({
            "handle": handle,
            "chart_type": "line",
            "x_col": "year",
            "y_cols": ["revenue"]
        });
        let result = handle_dataset_chart(&store, &args);
        assert_eq!(result["isError"], false);
        let schema = result["structuredContent"]["$schema"].as_str().unwrap_or("");
        assert!(
            schema.contains("vega-lite") && schema.contains("v5"),
            "expected Vega-Lite v5 $schema, got: {schema}"
        );
    }

    // ── AC-3: build_bi_asset returns bi-asset.v1 shape with no rows ──────────

    #[test]
    fn ac3_build_bi_asset_returns_asset_no_rows() {
        let (store, handle) = store_with_dataset();
        let args = json!({ "handle": handle });
        let result = handle_build_bi_asset(&store, &args);

        assert_eq!(result["isError"], false, "expected isError=false: {result:?}");
        let content = &result["structuredContent"];
        assert_eq!(content["asset"], "bi-asset.v1", "asset tag");
        assert!(
            !content["title"].as_str().unwrap_or("").is_empty(),
            "title should be non-empty"
        );
        assert!(
            !content["description"].as_str().unwrap_or("").is_empty(),
            "description should be non-empty"
        );
        assert!(content["vega_spec"].is_object(), "vega_spec should be an object");
        assert!(content["caveats"].is_array(), "caveats should be an array");
        assert!(content.get("rows").is_none(), "rows must NOT appear in the response");
    }

    // ── AC-4: dataset_chart returns isError=true for unknown x_col ───────────

    #[test]
    fn ac4_dataset_chart_unknown_x_col_returns_error() {
        let (store, handle) = store_with_dataset();
        let args = json!({
            "handle": handle,
            "chart_type": "bar",
            "x_col": "nonexistent_column",
            "y_cols": ["revenue"]
        });
        let result = handle_dataset_chart(&store, &args);
        assert_eq!(result["isError"], true);
        let code = result["structuredContent"]["error"]["code"].as_str().unwrap_or("");
        assert_eq!(code, "unknown_column");
    }

    // ── AC-5: dataset_chart returns isError=true for missing handle ───────────

    #[test]
    fn ac5_dataset_chart_missing_handle_returns_error() {
        let store = Store::new(0);
        let fake_handle = dh_spec::DatasetHandle {
            id: "hdl_doesnotexist".to_string(),
            created_at: 0,
            ttl_secs: 3600,
            derived_from: None,
        };
        let args = json!({
            "handle": fake_handle,
            "chart_type": "bar",
            "x_col": "year",
            "y_cols": ["revenue"]
        });
        let result = handle_dataset_chart(&store, &args);
        assert_eq!(result["isError"], true);
    }

    // ── AC-6: build_bi_asset returns isError=true for expired handle ──────────

    #[test]
    fn ac6_build_bi_asset_expired_handle_returns_error() {
        let store = Store::new(0);
        let ds = make_test_dataset();
        let handle = store.put(ds, 0); // expires immediately
        store.evict_expired();

        let args = json!({ "handle": handle });
        let result = handle_build_bi_asset(&store, &args);
        assert_eq!(result["isError"], true);
    }

    // ── AC-7: dataset_to_payload caps rows at CHART_MAX_ROWS ─────────────────

    #[test]
    fn ac7_dataset_to_payload_caps_at_chart_max_rows() {
        // Build a dataset with CHART_MAX_ROWS + 10 rows.
        let n = CHART_MAX_ROWS + 10;
        let col = make_schema("val", ColumnRole::Measure, DType::Float);
        let data: Vec<Option<f64>> = (0..n).map(|i| Some(i as f64)).collect();
        let ds = Dataset::new(vec![col], vec![ColumnData::Float(data)]).expect("valid");
        let store = Store::new(0);
        let handle = store.put(ds, 3600);

        let payload = dataset_to_payload(&store, &handle).expect("should succeed");
        let row_count = payload["rows"].as_array().map_or(0, Vec::len);
        assert!(
            row_count <= CHART_MAX_ROWS,
            "payload row count {row_count} exceeds CHART_MAX_ROWS={CHART_MAX_ROWS}"
        );
    }

    // ── AC-8: compose_dashboard returns a dashboard.v1 spec ──────────────────

    #[test]
    fn ac8_compose_dashboard_returns_dashboard_spec() {
        let store = Store::new(0);
        let ds1 = make_test_dataset();
        let ds2 = make_test_dataset();
        let h1 = store.put(ds1, 3600);
        let h2 = store.put(ds2, 3600);

        let args = json!({
            "title": "Test Dashboard",
            "handles": [h1, h2],
            "layout": "grid",
            "columns": 2
        });
        let result = handle_compose_dashboard(&store, &args);
        assert_eq!(result["isError"], false, "expected success: {result:?}");
        let content = &result["structuredContent"];
        assert_eq!(content["dashboard"], "dashboard.v1");
        assert_eq!(content["title"], "Test Dashboard");
        assert!(content.get("rows").is_none(), "rows must NOT appear in the response");
    }
}
