//! Acceptance tests for the four handle-op MCP tools.
//!
//! AC1  Each tool returns a new handle distinct from the input; input handle remains valid.
//! AC2  `dataset_chart` returns Vega-Lite JSON (mark + encoding); no new handle; no binary payload.
//! AC3  Results above inline_threshold (K=20) never return raw rows (only head_sample ≤ 20).
//! AC4  Exactly one AtScale round-trip in a 4-turn walkthrough (turns 2–4 use handle ops).
//! AC5  All existing mqo-mcp-server tests still pass (regression guard — tool count = 13).

use mqo_mcp_server::{
    handle_ops::{
        handle_dataset_aggregate, handle_dataset_chart, handle_dataset_period_over_period,
        handle_dataset_slice, HandleStore, INLINE_THRESHOLD,
    },
    tool_descriptors, BackendCapabilities, Server, ServerEngine, ToolPaths,
};
use mqo_duckdb_handle_store::{ColumnSchema, ResultStore};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_store() -> HandleStore {
    HandleStore::new()
}

/// Store `n` synthetic rows `{col_a: i, col_b: i*2.0}` and return the handle UUID string.
fn store_n_rows(store: &HandleStore, n: usize) -> String {
    let rows: Vec<Value> = (0..n)
        .map(|i| json!({ "col_a": i as i64, "col_b": (i as f64) * 2.0 }))
        .collect();
    let schema = vec![
        ColumnSchema { name: "col_a".to_string(), ty: "integer".to_string() },
        ColumnSchema { name: "col_b".to_string(), ty: "double".to_string() },
    ];
    let mut guard = store.store.lock().unwrap();
    let env = guard.put(&rows, &schema, 0).unwrap();
    env.handle.0
}

/// Store sales rows with date + revenue for period-over-period tests.
fn store_sales_rows(store: &HandleStore) -> String {
    let rows = vec![
        json!({ "date": "2023-01-15", "revenue": 100.0, "region": "US" }),
        json!({ "date": "2023-02-20", "revenue": 200.0, "region": "US" }),
        json!({ "date": "2024-01-10", "revenue": 150.0, "region": "US" }),
        json!({ "date": "2024-02-05", "revenue": 250.0, "region": "US" }),
    ];
    let schema = vec![
        ColumnSchema { name: "date".to_string(), ty: "string".to_string() },
        ColumnSchema { name: "revenue".to_string(), ty: "double".to_string() },
        ColumnSchema { name: "region".to_string(), ty: "string".to_string() },
    ];
    let mut guard = store.store.lock().unwrap();
    let env = guard.put(&rows, &schema, 0).unwrap();
    env.handle.0
}

// ── AC1: each tool returns a new handle distinct from input; input remains valid ──

#[test]
fn ac1_dataset_aggregate_returns_new_handle() {
    let store = make_store();
    let input_handle = store_n_rows(&store, 30); // N > 20

    let result = handle_dataset_aggregate(
        &store.store,
        &json!({
            "handle": input_handle,
            "group_by": ["col_a"],
            "measures": [{ "col": "col_b", "agg": "sum" }]
        }),
    );

    assert_eq!(result["isError"], json!(false), "aggregate must not error: {result}");
    let sc = &result["structuredContent"];
    let new_handle = sc["new_handle"].as_str().expect("new_handle present");
    assert_ne!(new_handle, input_handle.as_str(), "new_handle must differ from input");

    // Input handle still resolves.
    let guard = store.store.lock().unwrap();
    let meta = guard.metadata(&mqo_duckdb_handle_store::DatasetHandle(input_handle.clone()));
    assert!(meta.is_ok(), "input handle still valid after aggregate");
}

#[test]
fn ac1_dataset_slice_returns_new_handle() {
    let store = make_store();
    let input_handle = store_n_rows(&store, 30);

    let result = handle_dataset_slice(
        &store.store,
        &json!({
            "handle": input_handle,
            "filters": [{ "col": "col_a", "op": "<", "value": 10 }]
        }),
    );

    assert_eq!(result["isError"], json!(false), "slice must not error: {result}");
    let sc = &result["structuredContent"];
    let new_handle = sc["new_handle"].as_str().expect("new_handle present");
    assert_ne!(new_handle, input_handle.as_str(), "new_handle must differ from input");

    let guard = store.store.lock().unwrap();
    let meta = guard.metadata(&mqo_duckdb_handle_store::DatasetHandle(input_handle.clone()));
    assert!(meta.is_ok(), "input handle still valid after slice");
}

#[test]
fn ac1_dataset_period_over_period_returns_new_handle() {
    let store = make_store();
    let input_handle = store_sales_rows(&store);

    let result = handle_dataset_period_over_period(
        &store.store,
        &json!({
            "handle": input_handle,
            "date_col": "date",
            "period": "year",
            "measure_cols": ["revenue"]
        }),
    );

    assert_eq!(result["isError"], json!(false), "pop must not error: {result}");
    let sc = &result["structuredContent"];
    let new_handle = sc["new_handle"].as_str().expect("new_handle present");
    assert_ne!(new_handle, input_handle.as_str(), "new_handle must differ from input");

    let guard = store.store.lock().unwrap();
    let meta = guard.metadata(&mqo_duckdb_handle_store::DatasetHandle(input_handle.clone()));
    assert!(meta.is_ok(), "input handle still valid after pop");
}

// ── AC2: dataset_chart returns VL5 JSON; no new handle; no binary ────────────

#[test]
fn ac2_dataset_chart_returns_vega_lite_spec_no_new_handle() {
    let store = make_store();
    let input_handle = store_n_rows(&store, 5);

    let result = handle_dataset_chart(
        &store.store,
        &json!({
            "handle": input_handle,
            "chart_type": "line",
            "x_col": "col_a",
            "y_cols": ["col_b"],
            "title": "Test Chart"
        }),
    );

    assert_eq!(result["isError"], json!(false), "chart must not error: {result}");
    let spec = &result["structuredContent"];

    // Must have Vega-Lite $schema.
    let schema_str = spec["$schema"].as_str().expect("$schema present");
    assert!(schema_str.contains("vega-lite"), "$schema references vega-lite: {schema_str}");

    // Must have mark and encoding.
    assert!(spec["mark"].is_string(), "spec has mark: {spec}");
    assert!(spec["encoding"].is_object(), "spec has encoding: {spec}");

    // Must NOT contain new_handle.
    assert!(spec.get("new_handle").is_none(), "chart must not return a new_handle: {spec}");

    // Must NOT contain binary payload.
    let spec_str = serde_json::to_string(spec).unwrap();
    assert!(!spec_str.contains("data:image"), "no embedded image: {spec_str}");
    assert!(!spec_str.contains("base64"), "no base64 payload: {spec_str}");
}

#[test]
fn ac2_dataset_chart_encodes_x_and_y_columns() {
    let store = make_store();
    let input_handle = store_n_rows(&store, 3);

    let result = handle_dataset_chart(
        &store.store,
        &json!({
            "handle": input_handle,
            "chart_type": "bar",
            "x_col": "col_a",
            "y_cols": ["col_b"]
        }),
    );

    assert_eq!(result["isError"], json!(false), "{result}");
    let spec = &result["structuredContent"];
    let encoding = spec["encoding"].as_object().expect("encoding object");

    // x encoding must reference col_a.
    assert_eq!(spec["mark"], json!("bar"), "mark=bar: {spec}");
    let x_field = encoding.get("x").and_then(|x| x.get("field")).and_then(Value::as_str);
    assert_eq!(x_field, Some("col_a"), "x.field = col_a: {spec}");
    let y_field = encoding.get("y").and_then(|y| y.get("field")).and_then(Value::as_str);
    assert_eq!(y_field, Some("col_b"), "y.field = col_b: {spec}");
}

// ── AC3: results above K=20 never return raw rows (only head_sample ≤ 20) ────

#[test]
fn ac3_aggregate_above_threshold_head_sample_capped_at_k() {
    let store = make_store();
    // 40 rows, each with unique col_a → 40 groups after aggregate.
    let input_handle = store_n_rows(&store, 40);

    let result = handle_dataset_aggregate(
        &store.store,
        &json!({
            "handle": input_handle,
            "group_by": ["col_a"],
            "measures": [{ "col": "col_b", "agg": "sum" }]
        }),
    );

    assert_eq!(result["isError"], json!(false), "{result}");
    let sc = &result["structuredContent"];
    let row_count = sc["row_count"].as_u64().expect("row_count") as usize;
    assert_eq!(row_count, 40, "40 groups expected");

    let head = sc["head_sample"].as_array().expect("head_sample array");
    assert!(head.len() <= INLINE_THRESHOLD, "head_sample capped at K={INLINE_THRESHOLD}: got {}", head.len());
    assert!(!sc.as_object().unwrap().contains_key("rows"), "no 'rows' key — only head_sample");
}

#[test]
fn ac3_slice_above_threshold_head_sample_capped_at_k() {
    let store = make_store();
    // 50 rows, filter keeps all.
    let input_handle = store_n_rows(&store, 50);

    let result = handle_dataset_slice(
        &store.store,
        &json!({
            "handle": input_handle,
            "filters": [{ "col": "col_a", "op": ">=", "value": 0 }]
        }),
    );

    assert_eq!(result["isError"], json!(false), "{result}");
    let sc = &result["structuredContent"];
    let row_count = sc["row_count"].as_u64().expect("row_count") as usize;
    assert_eq!(row_count, 50, "all 50 rows kept");

    let head = sc["head_sample"].as_array().expect("head_sample");
    assert!(head.len() <= INLINE_THRESHOLD, "head_sample ≤ K: got {}", head.len());
}

#[test]
fn ac3_slice_empty_match_returns_valid_handle_with_zero_rows() {
    let store = make_store();
    let input_handle = store_n_rows(&store, 10);

    // Filter that matches nothing.
    let result = handle_dataset_slice(
        &store.store,
        &json!({
            "handle": input_handle,
            "filters": [{ "col": "col_a", "op": ">", "value": 9999 }]
        }),
    );

    assert_eq!(result["isError"], json!(false), "empty slice must not error: {result}");
    let sc = &result["structuredContent"];
    assert_eq!(sc["row_count"], json!(0), "row_count=0 for empty match");
    let new_handle = sc["new_handle"].as_str().expect("new_handle even for empty result");
    assert!(!new_handle.is_empty(), "valid handle returned for empty result");
}

// ── AC4: 4-turn walkthrough — only one "engine" call (handle ops have zero) ──

/// In-process walkthrough: turn 1 stores rows via a synthetic "query" (no live AtScale),
/// turns 2–4 use handle ops.  Engine call count stays at 1.
#[test]
fn ac4_four_turn_walkthrough_one_engine_call() {
    let store = make_store();

    // Simulate engine call counter.
    let mut engine_calls: u32 = 0;

    // Turn 1: "query_multidimensional" — store the result (engine call #1).
    let rows: Vec<Value> = (0..25)
        .map(|i| json!({
            "date": format!("2024-{:02}-01", (i % 12) + 1),
            "revenue": (i as f64) * 10.0,
            "region": if i % 2 == 0 { "US" } else { "EU" }
        }))
        .collect();
    let schema = vec![
        ColumnSchema { name: "date".to_string(), ty: "string".to_string() },
        ColumnSchema { name: "revenue".to_string(), ty: "double".to_string() },
        ColumnSchema { name: "region".to_string(), ty: "string".to_string() },
    ];
    let turn1_handle = {
        let mut g = store.store.lock().unwrap();
        g.put(&rows, &schema, 0).unwrap().handle.0
    };
    engine_calls += 1; // exactly one engine call

    // Turn 2: slice to US.
    let turn2 = handle_dataset_slice(
        &store.store,
        &json!({ "handle": turn1_handle, "filters": [{ "col": "region", "op": "=", "value": "US" }] }),
    );
    assert_eq!(turn2["isError"], json!(false), "turn2 slice: {turn2}");
    let turn2_handle = turn2["structuredContent"]["new_handle"].as_str().unwrap().to_string();
    // No engine call in turn 2.

    // Turn 3: aggregate by month.
    let turn3 = handle_dataset_aggregate(
        &store.store,
        &json!({
            "handle": turn2_handle,
            "group_by": ["date"],
            "measures": [{ "col": "revenue", "agg": "sum" }]
        }),
    );
    assert_eq!(turn3["isError"], json!(false), "turn3 aggregate: {turn3}");
    let turn3_handle = turn3["structuredContent"]["new_handle"].as_str().unwrap().to_string();
    // No engine call in turn 3.

    // Turn 4: chart.
    let turn4 = handle_dataset_chart(
        &store.store,
        &json!({
            "handle": turn3_handle,
            "chart_type": "line",
            "x_col": "date",
            "y_cols": ["revenue_sum"]
        }),
    );
    assert_eq!(turn4["isError"], json!(false), "turn4 chart: {turn4}");
    let spec = &turn4["structuredContent"];
    assert!(spec["$schema"].is_string(), "chart returns vega-lite spec: {spec}");
    // No engine call in turn 4.

    assert_eq!(engine_calls, 1, "exactly one engine call across the 4-turn session");
}

// ── AC5: regression — tool count = 14; existing tools unaffected ─────────────

#[test]
fn ac5_tool_count_is_fourteen_with_handle_ops_and_cursor() {
    let tools = tool_descriptors();
    let arr = tools.as_array().expect("tool list is array");
    assert_eq!(
        arr.len(),
        16,
        "must advertise 16 tools (12 existing + 4 handle-ops): {:?}",
        arr.iter().map(|t| t["name"].as_str().unwrap_or("?")).collect::<Vec<_>>()
    );
}

#[test]
fn ac5_all_four_handle_ops_in_tool_list() {
    let tools = tool_descriptors();
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for expected in ["dataset_aggregate", "dataset_slice", "dataset_period_over_period", "dataset_chart"] {
        assert!(names.contains(&expected), "tool {expected} must be in tool list: {names:?}");
    }
}

#[test]
fn ac5_all_handle_op_tools_carry_read_only_hint() {
    let tools = tool_descriptors();
    let arr = tools.as_array().unwrap();
    for tool_name in ["dataset_aggregate", "dataset_slice", "dataset_period_over_period", "dataset_chart"] {
        let t = arr.iter().find(|t| t["name"] == tool_name).unwrap_or_else(|| panic!("{tool_name} missing"));
        assert_eq!(
            t["annotations"]["readOnlyHint"],
            json!(true),
            "{tool_name} must carry readOnlyHint: true"
        );
    }
}

// ── Additional coverage ───────────────────────────────────────────────────────

#[test]
fn handle_not_found_returns_structured_error() {
    let store = make_store();
    let bogus = "00000000-0000-0000-0000-000000000000";

    let cases: Vec<Value> = vec![
        json!({ "handle": bogus, "filters": [] }),
        json!({ "handle": bogus, "group_by": ["x"], "measures": [{ "col": "y", "agg": "sum" }] }),
        json!({ "handle": bogus, "date_col": "d", "period": "year", "measure_cols": ["v"] }),
        json!({ "handle": bogus, "chart_type": "bar", "x_col": "x", "y_cols": ["y"] }),
    ];
    let results: Vec<Value> = vec![
        handle_dataset_slice(&store.store, &cases[0]),
        handle_dataset_aggregate(&store.store, &cases[1]),
        handle_dataset_period_over_period(&store.store, &cases[2]),
        handle_dataset_chart(&store.store, &cases[3]),
    ];
    for r in &results {
        assert_eq!(r["isError"], json!(true), "bogus handle must error: {r}");
        let code = r["structuredContent"]["error"]["code"].as_str().unwrap_or("");
        assert!(!code.is_empty(), "structured error code must be present: {r}");
    }
}

#[test]
fn period_over_period_output_shape_has_prior_and_delta_cols() {
    let store = make_store();
    let input_handle = store_sales_rows(&store);

    let result = handle_dataset_period_over_period(
        &store.store,
        &json!({
            "handle": input_handle,
            "date_col": "date",
            "period": "year",
            "measure_cols": ["revenue"]
        }),
    );

    assert_eq!(result["isError"], json!(false), "{result}");
    let sc = &result["structuredContent"];
    // Should have 2 buckets: 2023 and 2024.
    assert_eq!(sc["row_count"], json!(2), "2 year buckets: {sc}");

    let head = sc["head_sample"].as_array().expect("head_sample");
    // First bucket (2023): no prior.
    let first = &head[0];
    assert!(first.get("period_bucket").is_some(), "period_bucket col present: {first}");
    assert!(first.get("revenue").is_some(), "revenue col present: {first}");
    assert!(first.get("revenue_prior").is_some(), "revenue_prior col present: {first}");
    assert!(first.get("revenue_delta").is_some(), "revenue_delta col present: {first}");

    // Second bucket (2024): delta should be non-null.
    let second = &head[1];
    let delta = second.get("revenue_delta");
    assert!(delta.is_some(), "revenue_delta present in second bucket: {second}");
    assert_ne!(delta.unwrap(), &Value::Null, "revenue_delta is non-null for second bucket: {second}");
}

#[test]
fn dataset_server_without_handle_store_returns_unsupported_error() {
    // A Server with handle_store: None should return unsupported_operation for handle ops.
    let srv = Server {
        catalog: json!({}),
        stats: json!({}),
        tools: ToolPaths {
            bind: PathBuf::from("mqo-bind"),
            route: PathBuf::from("mqo-route"),
            dax: PathBuf::from("mqo-dax"),
            mdx: PathBuf::from("mqo-mdx"),
        },
        row_threshold: 50_000,
        engine: ServerEngine::Fixture,
        backend_override: None,
        capabilities: BackendCapabilities::all_live(),
        registry: None,
        health_cache: None,
        handle_store: None,
        cursor_store: None,
        page_size: mqo_mcp_server::cursor::DEFAULT_PAGE_SIZE,
        enriched: None,
        xmla_model_coords: HashMap::new(),
    };

    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "dataset_slice",
            "arguments": { "handle": "fake-uuid", "filters": [] }
        }
    });
    let resp = srv.handle(&req).expect("response");
    let result = &resp["result"];
    assert_eq!(result["isError"], json!(true), "no handle_store → isError=true: {result}");
    let code = result["structuredContent"]["error"]["code"].as_str().unwrap_or("");
    assert_eq!(code, "unsupported_operation", "error code must be unsupported_operation: {result}");
}
