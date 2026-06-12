//! End-to-end scenario tests: NLQ → BI visualization.
//!
//! Each test starts from an MQO (what an LLM would construct for the NLQ in
//! the doc-comment) and drives the full tool chain:
//!   1. `recommend_chart { rows, bound }` → assert mark
//!   2. `build_vega_spec { recommendation, rows }` → assert Vega spec shape
//!   3. `mqo_bi_asset_bundle::build_asset` → assert title, caveats
//!
//! Steps 1–3 always run using a synthetic fixture response against
//! `fixtures/catalog.json`. Step 0 (`query_multidimensional`) is fleet-gated
//! and tested only in the separate `binary_stdio_test.rs` full-chain test.
//!
//! AC1  `e2e_total_revenue_kpi` — mark "big_number", Vega "text", title "(total)"
//! AC2  `e2e_revenue_by_year`   — mark "line", title "Revenue by Year"
//! AC3  `e2e_revenue_by_country` — mark "bar", title "Revenue by Country"
//! AC4  `e2e_revenue_by_account_high_cardinality` — mark "bar", clutter caveat
//! AC5  `e2e_revenue_vs_units_sold` — mark "point", title "Revenue and Units Sold"
//! AC6  `e2e_balance_by_year_semi_additive` — mark "line", semi-additive caveat
//! AC7  `e2e_margin_pct_by_country_calc_caveat` — mark "bar", calc-% caveat

use mqo_bi_asset_bundle::build_asset;
use mqo_mcp_server::{BackendCapabilities, Server, ServerEngine, ToolPaths};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ── Harness helpers ───────────────────────────────────────────────────────────

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn load_catalog() -> Value {
    let p = fixtures_dir().join("catalog.json");
    serde_json::from_str(&std::fs::read_to_string(p).expect("read catalog")).expect("parse catalog")
}

fn load_stats() -> Value {
    let p = fixtures_dir().join("stats.json");
    serde_json::from_str(&std::fs::read_to_string(p).expect("read stats")).expect("parse stats")
}

fn sibling_release_dir(crate_name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(crate_name)
        .join("target/release")
}

fn find_bin(bin: &str, crate_name: &str) -> PathBuf {
    let sib = sibling_release_dir(crate_name).join(bin);
    if sib.exists() {
        return sib;
    }
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join(".local/bin").join(bin);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from(bin)
}

fn resolve_tools() -> ToolPaths {
    ToolPaths {
        bind: find_bin("mqo-bind", "mqo-catalog-binder"),
        route: find_bin("mqo-route", "mqo-backend-router"),
        dax: find_bin("mqo-dax", "mqo-dax-compiler"),
        mdx: find_bin("mqo-mdx", "mqo-mdx-compiler"),
    }
}

fn plain_server() -> Server {
    Server {
        catalog: load_catalog(),
        stats: load_stats(),
        tools: resolve_tools(),
        row_threshold: 50_000,
        engine: ServerEngine::Fixture,
        backend_override: None,
        capabilities: BackendCapabilities::all_live(),
        registry: None,
        health_cache: None,
        handle_store: None,
        cursor_store: None,
        page_size: mqo_mcp_server::cursor::DEFAULT_PAGE_SIZE,
        inline_threshold: mqo_mcp_server::INLINE_THRESHOLD,
        enriched: None,
        xmla_model_coords: HashMap::new(),
    }
}

fn call_tool(srv: &Server, name: &str, arguments: Value) -> Value {
    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    });
    srv.handle(&req)
        .expect("handle returned None")
        .get("result")
        .cloned()
        .expect("result present")
}

/// Build a synthetic `{ rows, bound }` response for the profiler / chart tools.
fn synthetic_response(
    measures: &[&str],
    dimensions: &[&str],
    rows: Vec<Value>,
) -> Value {
    json!({
        "rows": rows,
        "bound": {
            "measures": measures,
            "dimensions": dimensions
        }
    })
}

/// Assert the Vega-Lite spec in `result["structuredContent"]` has the expected shape.
fn assert_vega_spec(result: &Value, expected_vega_mark: &str, context: &str) {
    assert!(
        !result.get("isError").and_then(Value::as_bool).unwrap_or(true),
        "{context}: isError must be false, got: {result}"
    );
    let spec = result.get("structuredContent").unwrap_or(result);
    assert!(
        spec.get("$schema")
            .and_then(Value::as_str)
            .is_some_and(|s| s.contains("vega-lite")),
        "{context}: $schema must contain 'vega-lite', got: {spec}"
    );
    assert_eq!(
        spec.get("mark").and_then(Value::as_str),
        Some(expected_vega_mark),
        "{context}: Vega mark mismatch"
    );
    assert!(
        spec.get("data")
            .and_then(|d| d.get("values"))
            .and_then(Value::as_array)
            .is_some(),
        "{context}: data.values must be an array"
    );
}

// ── AC1: total revenue KPI ────────────────────────────────────────────────────

/// NLQ: "What is total revenue?"
#[test]
fn e2e_total_revenue_kpi() {
    let srv = plain_server();
    let catalog = load_catalog();

    let response = synthetic_response(
        &["sales.revenue"],
        &[],
        vec![json!({ "sales.revenue": 1_234_567.0 })],
    );

    let rec_result = call_tool(
        &srv,
        "recommend_chart",
        json!({ "rows": response["rows"], "bound": response["bound"] }),
    );
    assert!(
        !rec_result.get("isError").and_then(Value::as_bool).unwrap_or(true),
        "recommend_chart isError must be false: {rec_result}"
    );
    let rec_sc = rec_result.get("structuredContent").expect("structuredContent");
    assert_eq!(
        rec_sc.get("mark").and_then(Value::as_str),
        Some("big_number"),
        "AC1: recommend_chart mark must be 'big_number'"
    );

    let vega_result = call_tool(
        &srv,
        "build_vega_spec",
        json!({ "recommendation": rec_sc, "rows": response["rows"] }),
    );
    assert_vega_spec(&vega_result, "text", "AC1 build_vega_spec");

    let asset = build_asset(&response, &catalog).expect("build_asset AC1");
    assert!(
        asset.title.contains("total"),
        "AC1: title must contain 'total', got: {:?}",
        asset.title
    );
    assert!(
        asset.vega_spec.get("$schema")
            .and_then(Value::as_str)
            .is_some_and(|s| s.contains("vega-lite")),
        "AC1: vega_spec.$schema must contain 'vega-lite'"
    );
    assert!(asset.caveats.is_empty(), "AC1: no caveats expected, got: {:?}", asset.caveats);
}

// ── AC2: revenue by year ──────────────────────────────────────────────────────

/// NLQ: "Show revenue by year"
#[test]
fn e2e_revenue_by_year() {
    let srv = plain_server();
    let catalog = load_catalog();

    let response = synthetic_response(
        &["sales.revenue"],
        &["time.calendar.[Year]"],
        vec![
            json!({ "sales.revenue": 100.0, "time.calendar.[Year]": "2021" }),
            json!({ "sales.revenue": 120.0, "time.calendar.[Year]": "2022" }),
            json!({ "sales.revenue": 140.0, "time.calendar.[Year]": "2023" }),
        ],
    );

    let rec_result = call_tool(
        &srv,
        "recommend_chart",
        json!({ "rows": response["rows"], "bound": response["bound"] }),
    );
    let rec_sc = rec_result.get("structuredContent").expect("structuredContent");
    assert_eq!(
        rec_sc.get("mark").and_then(Value::as_str),
        Some("line"),
        "AC2: mark must be 'line'"
    );

    let vega_result = call_tool(
        &srv,
        "build_vega_spec",
        json!({ "recommendation": rec_sc, "rows": response["rows"] }),
    );
    assert_vega_spec(&vega_result, "line", "AC2 build_vega_spec");

    let vega_sc = vega_result.get("structuredContent").expect("structuredContent");
    let encoding = vega_sc.get("encoding").expect("encoding");
    assert!(
        encoding.get("x").is_some() || encoding.get("y").is_some(),
        "AC2: encoding must have at least one channel"
    );

    let asset = build_asset(&response, &catalog).expect("build_asset AC2");
    assert_eq!(asset.title, "Revenue by Year", "AC2: title mismatch");
    assert!(asset.caveats.is_empty(), "AC2: no caveats expected, got: {:?}", asset.caveats);
}

// ── AC3: revenue by country ───────────────────────────────────────────────────

/// NLQ: "Revenue by country"
#[test]
fn e2e_revenue_by_country() {
    let srv = plain_server();
    let catalog = load_catalog();

    let response = synthetic_response(
        &["sales.revenue"],
        &["geo.country.[Country]"],
        vec![
            json!({ "sales.revenue": 500.0, "geo.country.[Country]": "USA" }),
            json!({ "sales.revenue": 300.0, "geo.country.[Country]": "UK" }),
            json!({ "sales.revenue": 200.0, "geo.country.[Country]": "DE" }),
        ],
    );

    let rec_result = call_tool(
        &srv,
        "recommend_chart",
        json!({ "rows": response["rows"], "bound": response["bound"] }),
    );
    let rec_sc = rec_result.get("structuredContent").expect("structuredContent");
    assert_eq!(
        rec_sc.get("mark").and_then(Value::as_str),
        Some("bar"),
        "AC3: mark must be 'bar'"
    );

    let vega_result = call_tool(
        &srv,
        "build_vega_spec",
        json!({ "recommendation": rec_sc, "rows": response["rows"] }),
    );
    assert_vega_spec(&vega_result, "bar", "AC3 build_vega_spec");

    let asset = build_asset(&response, &catalog).expect("build_asset AC3");
    assert_eq!(asset.title, "Revenue by Country", "AC3: title mismatch");
    assert!(asset.caveats.is_empty(), "AC3: no caveats expected, got: {:?}", asset.caveats);
}

// ── AC4: revenue by account — high cardinality ────────────────────────────────

/// NLQ: "Revenue by account"
#[test]
fn e2e_revenue_by_account_high_cardinality() {
    let srv = plain_server();
    let catalog = load_catalog();

    // 30 distinct accounts → cardinality > 25 → clutter caveat
    let rows: Vec<Value> = (0..30)
        .map(|i| {
            json!({
                "sales.revenue": (i + 1) as f64 * 10.0,
                "customer.account.[Account]": format!("ACCT-{i:03}")
            })
        })
        .collect();

    let response = synthetic_response(
        &["sales.revenue"],
        &["customer.account.[Account]"],
        rows.clone(),
    );

    let rec_result = call_tool(
        &srv,
        "recommend_chart",
        json!({ "rows": response["rows"], "bound": response["bound"] }),
    );
    let rec_sc = rec_result.get("structuredContent").expect("structuredContent");
    // High cardinality does NOT change the primary mark — Bar always for 1m+1nominal
    assert_eq!(
        rec_sc.get("mark").and_then(Value::as_str),
        Some("bar"),
        "AC4: mark must be 'bar' (high-cardinality nominal does not change primary mark)"
    );

    let vega_result = call_tool(
        &srv,
        "build_vega_spec",
        json!({ "recommendation": rec_sc, "rows": response["rows"] }),
    );
    assert_vega_spec(&vega_result, "bar", "AC4 build_vega_spec");

    let asset = build_asset(&response, &catalog).expect("build_asset AC4");
    assert_eq!(asset.title, "Revenue by Account", "AC4: title mismatch");
    assert!(
        !asset.caveats.is_empty(),
        "AC4: high-cardinality clutter caveat must be present"
    );
    assert!(
        asset.caveats.iter().any(|c| c.contains("Account") && c.contains("categories")),
        "AC4: caveat must mention 'Account' and 'categories', got: {:?}",
        asset.caveats
    );
}

// ── AC5: revenue vs units sold ────────────────────────────────────────────────

/// NLQ: "Revenue vs units sold"
#[test]
fn e2e_revenue_vs_units_sold() {
    let srv = plain_server();
    let catalog = load_catalog();

    let response = synthetic_response(
        &["sales.revenue", "sales.units_sold"],
        &[],
        vec![
            json!({ "sales.revenue": 100.0, "sales.units_sold": 50.0 }),
            json!({ "sales.revenue": 200.0, "sales.units_sold": 80.0 }),
        ],
    );

    let rec_result = call_tool(
        &srv,
        "recommend_chart",
        json!({ "rows": response["rows"], "bound": response["bound"] }),
    );
    let rec_sc = rec_result.get("structuredContent").expect("structuredContent");
    assert_eq!(
        rec_sc.get("mark").and_then(Value::as_str),
        Some("point"),
        "AC5: mark must be 'point' for 2 measures with no dimensions"
    );

    let vega_result = call_tool(
        &srv,
        "build_vega_spec",
        json!({ "recommendation": rec_sc, "rows": response["rows"] }),
    );
    assert_vega_spec(&vega_result, "point", "AC5 build_vega_spec");

    let asset = build_asset(&response, &catalog).expect("build_asset AC5");
    assert!(
        asset.title.contains("Revenue") && asset.title.contains("Units Sold"),
        "AC5: title must contain 'Revenue' and 'Units Sold', got: {:?}",
        asset.title
    );
    assert!(asset.caveats.is_empty(), "AC5: no caveats expected, got: {:?}", asset.caveats);
}

// ── AC6: balance by year — semi-additive caveat ───────────────────────────────

/// NLQ: "Balance by year"
#[test]
fn e2e_balance_by_year_semi_additive() {
    let srv = plain_server();
    let catalog = load_catalog();

    let response = synthetic_response(
        &["sales.balance"],
        &["time.calendar.[Year]"],
        vec![
            json!({ "sales.balance": 10_000.0, "time.calendar.[Year]": "2021" }),
            json!({ "sales.balance": 12_000.0, "time.calendar.[Year]": "2022" }),
        ],
    );

    let rec_result = call_tool(
        &srv,
        "recommend_chart",
        json!({ "rows": response["rows"], "bound": response["bound"] }),
    );
    let rec_sc = rec_result.get("structuredContent").expect("structuredContent");
    assert_eq!(
        rec_sc.get("mark").and_then(Value::as_str),
        Some("line"),
        "AC6: mark must be 'line'"
    );

    let vega_result = call_tool(
        &srv,
        "build_vega_spec",
        json!({ "recommendation": rec_sc, "rows": response["rows"] }),
    );
    assert_vega_spec(&vega_result, "line", "AC6 build_vega_spec");

    let asset = build_asset(&response, &catalog).expect("build_asset AC6");
    assert_eq!(asset.title, "Balance by Year", "AC6: title mismatch");
    assert!(
        !asset.caveats.is_empty(),
        "AC6: semi-additive caveat must be present"
    );
    assert!(
        asset.caveats.iter().any(|c| c.contains("Balance") && c.contains("semi-additive")),
        "AC6: caveat must mention 'Balance' and 'semi-additive', got: {:?}",
        asset.caveats
    );
}

// ── AC7: margin % by country — calculated-measure caveat ─────────────────────

/// NLQ: "Margin % by country"
#[test]
fn e2e_margin_pct_by_country_calc_caveat() {
    let srv = plain_server();
    let catalog = load_catalog();

    let response = synthetic_response(
        &["sales.margin_pct"],
        &["geo.country.[Country]"],
        vec![
            json!({ "sales.margin_pct": 22.5, "geo.country.[Country]": "USA" }),
            json!({ "sales.margin_pct": 19.8, "geo.country.[Country]": "UK" }),
        ],
    );

    let rec_result = call_tool(
        &srv,
        "recommend_chart",
        json!({ "rows": response["rows"], "bound": response["bound"] }),
    );
    let rec_sc = rec_result.get("structuredContent").expect("structuredContent");
    assert_eq!(
        rec_sc.get("mark").and_then(Value::as_str),
        Some("bar"),
        "AC7: mark must be 'bar'"
    );

    let vega_result = call_tool(
        &srv,
        "build_vega_spec",
        json!({ "recommendation": rec_sc, "rows": response["rows"] }),
    );
    assert_vega_spec(&vega_result, "bar", "AC7 build_vega_spec");

    let asset = build_asset(&response, &catalog).expect("build_asset AC7");
    assert_eq!(asset.title, "Margin % by Country", "AC7: title mismatch");
    assert!(
        !asset.caveats.is_empty(),
        "AC7: calculated-percentage caveat must be present"
    );
    assert!(
        asset.caveats.iter().any(|c| c.contains("calculated percentage")),
        "AC7: caveat must mention 'calculated percentage', got: {:?}",
        asset.caveats
    );
}
