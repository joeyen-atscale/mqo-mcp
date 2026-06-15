//! Acceptance tests for the dh-store/dh-ops-backed handle-op MCP tools.
//!
//! After the merge (PRD-mqo-mcp-handle-merge), `handle_ops.rs` is backed by
//! `dh-store` + `dh-ops` (typed columnar). Each op derives a new handle and the
//! response is size-gated: raw `rows` are inlined only when
//! `row_count <= inline_threshold`.
//!
//! AC-1  Each op returns a new handle distinct from the input; input remains valid.
//! AC-2  ≤K rows → response includes inline `rows`.
//! AC-3  >K rows → response carries summary + handle + row_count and NO `rows`.
//! AC-5  Full 10-op `dataset_*` family is exposed; all carry readOnlyHint:true.

use mqo_mcp_server::{
    handle_ops::{
        handle_dataset_aggregate, handle_dataset_chart, handle_dataset_compare,
        handle_dataset_describe, handle_dataset_drill, handle_dataset_filter,
        handle_dataset_period_over_period, handle_dataset_pivot, handle_dataset_slice,
        handle_dataset_sort, handle_dataset_top_n, HandleStore, INLINE_THRESHOLD,
    },
    tool_descriptors,
};
use serde_json::{json, Value};

// ── Helpers ──────────────────────────────────────────────────────────────────

fn make_store() -> HandleStore {
    HandleStore::new()
}

/// Store `n` synthetic rows `{col_a: i, col_b: i*2.0, region}` → handle id string.
fn store_n_rows(store: &HandleStore, n: usize) -> String {
    let rows: Vec<Value> = (0..n)
        .map(|i| {
            json!({
                "col_a": i as i64,
                "col_b": (i as f64) * 2.0,
                "region": if i % 2 == 0 { "US" } else { "EU" }
            })
        })
        .collect();
    store.put_rows(&rows).expect("put_rows").id
}

fn sc(v: &Value) -> &Value {
    &v["structuredContent"]
}

// ── AC-1: each op returns a new, distinct handle; input remains valid ──────────

#[test]
fn ac1_aggregate_returns_new_handle_and_input_survives() {
    let store = make_store();
    let input = store_n_rows(&store, 30);

    let result = handle_dataset_aggregate(
        &store.store,
        &json!({ "handle": input, "group_by": ["region"], "agg": "sum", "measure": "col_b" }),
        INLINE_THRESHOLD,
        None, // test fixture: no catalog → guard fails-open
    );
    assert_eq!(result["isError"], json!(false), "aggregate must not error: {result}");
    let new_handle = sc(&result)["new_handle"].as_str().expect("new_handle present");
    assert_ne!(new_handle, input.as_str(), "new_handle must differ from input");

    // 2 regions → 2 groups; ≤K so rows inline.
    assert_eq!(sc(&result)["row_count"], json!(2));
    assert!(sc(&result).get("rows").is_some(), "small result inlines rows");
}

#[test]
fn ac1_legacy_aggregate_measures_shape_still_works() {
    let store = make_store();
    let input = store_n_rows(&store, 10);
    let result = handle_dataset_aggregate(
        &store.store,
        &json!({
            "handle": input,
            "group_by": ["region"],
            "measures": [{ "col": "col_b", "agg": "sum" }]
        }),
        INLINE_THRESHOLD,
        None, // test fixture: no catalog → guard fails-open
    );
    assert_eq!(result["isError"], json!(false), "legacy measures shape: {result}");
    assert_eq!(sc(&result)["row_count"], json!(2));
}

// ── AC-2: small result (≤K) inlines rows ───────────────────────────────────────

#[test]
fn ac2_small_slice_inlines_rows() {
    let store = make_store();
    let input = store_n_rows(&store, 10);
    let result = handle_dataset_slice(
        &store.store,
        &json!({ "handle": input, "filters": [{ "col": "region", "op": "=", "value": "US" }] }),
        INLINE_THRESHOLD,
    );
    assert_eq!(result["isError"], json!(false), "{result}");
    let rows = sc(&result)["rows"].as_array().expect("rows inline for small result");
    assert_eq!(rows.len(), 5, "5 US rows of 10");
}

// ── AC-3: large result (>K) gated — handle + summary, no rows ──────────────────

#[test]
fn ac3_large_sort_has_handle_summary_no_rows() {
    let store = make_store();
    let input = store_n_rows(&store, 40); // > K=25

    let result = handle_dataset_sort(
        &store.store,
        &json!({ "handle": input, "keys": [{ "col": "col_a", "dir": "desc" }] }),
        INLINE_THRESHOLD,
    );
    assert_eq!(result["isError"], json!(false), "{result}");
    let payload = sc(&result);
    assert_eq!(payload["row_count"], json!(40));
    assert!(payload.get("summary").is_some(), "summary present");
    assert!(payload["new_handle"].is_string(), "handle present");
    assert!(
        !payload.as_object().unwrap().contains_key("rows"),
        "NO rows above threshold: {payload}"
    );
}

// ── AC-5: filter / sort / top_n / pivot / compare / drill / describe ────────────

#[test]
fn filter_predicate_works() {
    let store = make_store();
    let input = store_n_rows(&store, 10);
    let result = handle_dataset_filter(
        &store.store,
        &json!({ "handle": input, "predicate": { "col": "region", "op": "eq", "val": "EU" } }),
        INLINE_THRESHOLD,
    );
    assert_eq!(result["isError"], json!(false), "{result}");
    assert_eq!(sc(&result)["row_count"], json!(5));
}

#[test]
fn top_n_works() {
    let store = make_store();
    let input = store_n_rows(&store, 10);
    let result = handle_dataset_top_n(
        &store.store,
        &json!({ "handle": input, "n": 3, "measure": "col_b", "dir": "top" }),
        INLINE_THRESHOLD,
    );
    assert_eq!(result["isError"], json!(false), "{result}");
    assert_eq!(sc(&result)["row_count"], json!(3));
}

#[test]
fn pivot_works() {
    let store = make_store();
    let rows: Vec<Value> = (0..12)
        .map(|i| {
            json!({
                "region": if i % 2 == 0 { "US" } else { "EU" },
                "parity": if (i / 2) % 2 == 0 { "lo" } else { "hi" },
                "col_b": i as f64
            })
        })
        .collect();
    let input = store.put_rows(&rows).unwrap().id;
    let result = handle_dataset_pivot(
        &store.store,
        &json!({ "handle": input, "row_dim": "region", "col_dim": "parity", "measure": "col_b", "agg": "sum" }),
        INLINE_THRESHOLD,
    );
    assert_eq!(result["isError"], json!(false), "{result}");
    assert!(sc(&result)["row_count"].as_u64().unwrap() >= 1);
}

#[test]
fn describe_works() {
    let store = make_store();
    let input = store_n_rows(&store, 10);
    let result = handle_dataset_describe(&store.store, &json!({ "handle": input }), INLINE_THRESHOLD);
    assert_eq!(result["isError"], json!(false), "{result}");
    // one row per column (col_a, col_b, region) = 3
    assert_eq!(sc(&result)["row_count"], json!(3));
}

#[test]
fn compare_two_handles() {
    let store = make_store();
    let a_rows = vec![json!({ "region": "US", "rev": 100.0 }), json!({ "region": "EU", "rev": 50.0 })];
    let b_rows = vec![json!({ "region": "US", "rev": 120.0 }), json!({ "region": "EU", "rev": 40.0 })];
    let a = store.put_rows(&a_rows).unwrap();
    let b = store.put_rows(&b_rows).unwrap();
    let result = handle_dataset_compare(
        &store.store,
        &json!({ "handle": a.id, "handle_b": b, "join_keys": ["region"], "measure": "rev" }),
        INLINE_THRESHOLD,
    );
    assert_eq!(result["isError"], json!(false), "compare: {result}");
    assert_eq!(sc(&result)["row_count"], json!(2));
}

#[test]
fn drill_expands_grouped_row() {
    let store = make_store();
    let input = store_n_rows(&store, 10);
    let agg = handle_dataset_aggregate(
        &store.store,
        &json!({ "handle": input, "group_by": ["region"], "agg": "sum", "measure": "col_b" }),
        INLINE_THRESHOLD,
        None, // test fixture: no catalog → guard fails-open
    );
    let agg_handle = sc(&agg)["new_handle"].as_str().unwrap().to_string();
    let result = handle_dataset_drill(
        &store.store,
        &json!({ "handle": agg_handle, "group_row": { "region": "US" } }),
        INLINE_THRESHOLD,
    );
    assert_eq!(result["isError"], json!(false), "drill: {result}");
    assert_eq!(sc(&result)["row_count"], json!(5));
}

// ── period_over_period + chart ─────────────────────────────────────────────────

#[test]
fn period_over_period_has_prior_and_delta() {
    let store = make_store();
    let rows = vec![
        json!({ "date": "2023-01-15", "revenue": 100.0 }),
        json!({ "date": "2024-02-05", "revenue": 250.0 }),
    ];
    let input = store.put_rows(&rows).unwrap().id;
    let result = handle_dataset_period_over_period(
        &store.store,
        &json!({ "handle": input, "date_col": "date", "period": "year", "measure_cols": ["revenue"] }),
        INLINE_THRESHOLD,
    );
    assert_eq!(result["isError"], json!(false), "{result}");
    assert_eq!(sc(&result)["row_count"], json!(2));
    let head = sc(&result)["rows"].as_array().expect("rows inline (2 ≤ K)");
    assert!(head[1].get("revenue_delta").is_some(), "delta col present");
    assert_ne!(head[1]["revenue_delta"], Value::Null, "second bucket delta non-null");
}

#[test]
fn chart_returns_vega_spec_no_new_handle() {
    let store = make_store();
    let input = store_n_rows(&store, 5);
    let result = handle_dataset_chart(
        &store.store,
        &json!({ "handle": input, "chart_type": "line", "x_col": "col_a", "y_cols": ["col_b"], "title": "T" }),
        INLINE_THRESHOLD,
    );
    assert_eq!(result["isError"], json!(false), "{result}");
    let spec = sc(&result);
    assert!(spec["$schema"].as_str().unwrap().contains("vega-lite"));
    assert!(spec.get("new_handle").is_none(), "chart mints no handle");
}

// ── error handling ─────────────────────────────────────────────────────────────

#[test]
fn handle_not_found_is_structured_error() {
    let store = make_store();
    let result = handle_dataset_sort(
        &store.store,
        &json!({ "handle": "hdl_doesnotexist", "keys": [{ "col": "x", "dir": "asc" }] }),
        INLINE_THRESHOLD,
    );
    assert_eq!(result["isError"], json!(true), "{result}");
    let code = sc(&result)["error"]["code"].as_str().unwrap_or("");
    assert!(!code.is_empty(), "structured error code present");
}

// ── tool-list surface (AC-1 union / FR-4) ──────────────────────────────────────

const DATASET_OPS: [&str; 11] = [
    "dataset_aggregate",
    "dataset_filter",
    "dataset_sort",
    "dataset_top_n",
    "dataset_pivot",
    "dataset_compare",
    "dataset_drill",
    "dataset_describe",
    "dataset_slice",
    "dataset_period_over_period",
    "dataset_chart",
];

#[test]
fn full_dataset_op_family_in_tool_list() {
    let tools = tool_descriptors();
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for expected in DATASET_OPS {
        assert!(names.contains(&expected), "tool {expected} missing: {names:?}");
    }
}

#[test]
fn all_dataset_ops_carry_read_only_hint() {
    let tools = tool_descriptors();
    let arr = tools.as_array().unwrap();
    for tool_name in DATASET_OPS {
        let t = arr
            .iter()
            .find(|t| t["name"] == tool_name)
            .unwrap_or_else(|| panic!("{tool_name} missing"));
        assert_eq!(
            t["annotations"]["readOnlyHint"],
            json!(true),
            "{tool_name} must carry readOnlyHint:true"
        );
    }
}

#[test]
fn core_tools_still_present() {
    let tools = tool_descriptors();
    let names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for expected in [
        "list_models",
        "describe_model",
        "search_columns",
        "query_multidimensional",
        "next_page",
        "health_status",
        "list_clusters",
        "diff_clusters",
        "recommend_chart",
        "build_vega_spec",
        "build_bi_asset",
        "compose_dashboard",
    ] {
        assert!(names.contains(&expected), "core tool {expected} missing: {names:?}");
    }
}
