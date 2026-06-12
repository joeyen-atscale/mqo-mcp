//! Guardrail-stack acceptance tests.
//!
//! AC1  `describe_model` with enrichment annotates measures with
//!      `compatible_hierarchies`.
//! AC2  `describe_model` without enrichment omits `compatible_hierarchies`
//!      entirely (never `null`, never empty array — just absent).
//! AC3  Each `compatible_hierarchies` entry has `hierarchy_unique_name`
//!      and `level_unique_names` keys with correct types.
//! AC4  `query_multidimensional` response always includes `filters_applied`
//!      and `filters_dropped` keys (even when both are empty arrays).
//! AC5  `ServerEnrichedData::from_json` with valid enriched JSON returns `Some`.
//! AC6  `ServerEnrichedData::from_json` with a JSON object that has no
//!      `columns` key returns `None` (graceful fail-open).
//! AC7  Auto-derivation from the tpcds fixture catalog produces a non-empty
//!      `compatible_hierarchies` map (≥1 measure entry, each with ≥1 hierarchy).
//! AC8  `catalog_json` in the enriched data is a valid JSON string that
//!      round-trips through `serde_json`.
//! AC9  Per-measure `compatible_hierarchies` count matches measures in the
//!      enriched catalog.
//! AC10 `CrossFactIncompatible` `PipelineError` variant carries the correct
//!      report shape: `incompatible` array at the root of `report`.
//!
//! Fleet-gated ACs (AC4) are skipped with a note when the fleet binaries
//! are not present.

use mqo_mcp_server::{BackendCapabilities, PipelineError, Server, ServerEnrichedData, ServerEngine, ToolPaths};
use mqoguard_column_group_enrichment::{enrich, CatalogSnapshot, FactBindings};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ── Harness helpers ──────────────────────────────────────────────────────────

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn load_tpcds_catalog() -> Value {
    let p = fixtures_dir().join("tpcds_catalog.json");
    serde_json::from_str(&std::fs::read_to_string(p).expect("read tpcds_catalog"))
        .expect("parse tpcds_catalog")
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

fn fleet_present() -> bool {
    let t = resolve_tools();
    [&t.bind, &t.route, &t.dax, &t.mdx]
        .iter()
        .all(|p| p.exists())
}

/// Run the enrichment pipeline on the tpcds catalog and build `ServerEnrichedData`
/// from the enriched output — mirrors `try_auto_derive_enriched` in `main.rs`.
fn tpcds_enriched() -> ServerEnrichedData {
    let catalog = load_tpcds_catalog();
    let snap: CatalogSnapshot =
        serde_json::from_value(catalog).expect("tpcds catalog parses as CatalogSnapshot");
    let bindings = FactBindings::tpcds_defaults();
    let enriched = enrich(&snap, &bindings);
    let enriched_json = serde_json::to_value(&enriched).expect("serialize enriched catalog");
    ServerEnrichedData::from_json(&enriched_json)
        .expect("enriched tpcds catalog must build ServerEnrichedData")
}

/// Server backed by the tpcds catalog with enrichment derived and wired in.
fn enriched_server() -> Server {
    let catalog = load_tpcds_catalog();
    Server {
        catalog,
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
        enriched: Some(Arc::new(tpcds_enriched())),
        xmla_model_coords: HashMap::new(),
    }
}

/// Server with the small sales catalog and no enrichment.
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

/// A minimal valid MQO against the small sales catalog (catalog.json).
fn sales_mqo(limit: u64) -> Value {
    json!({
        "model": "sales",
        "measures": [{ "unique_name": "Revenue" }],
        "dimensions": [],
        "filters": [],
        "time_intelligence": [],
        "order": null,
        "limit": limit,
        "non_empty": true
    })
}

// ── AC1 ──────────────────────────────────────────────────────────────────────

#[test]
fn ac1_describe_model_with_enrichment_annotates_measures() {
    let srv = enriched_server();
    let result = call_tool(
        &srv,
        "describe_model",
        json!({ "model": "tpcds_benchmark_model" }),
    );

    let columns = result["structuredContent"]["columns"]
        .as_array()
        .expect("columns array present");

    let measures: Vec<&Value> = columns
        .iter()
        .filter(|c| c.get("kind").and_then(Value::as_str) == Some("measure"))
        .collect();

    assert!(!measures.is_empty(), "at least one measure in tpcds catalog");

    // Every measure must have compatible_hierarchies injected.
    for m in &measures {
        let un = m["unique_name"].as_str().unwrap_or("?");
        assert!(
            m.get("compatible_hierarchies").is_some(),
            "measure {un} missing compatible_hierarchies"
        );
    }
}

// ── AC2 ──────────────────────────────────────────────────────────────────────

#[test]
fn ac2_describe_model_without_enrichment_omits_compatible_hierarchies() {
    let srv = plain_server();
    let result = call_tool(&srv, "describe_model", json!({}));

    let columns = result["structuredContent"]["columns"]
        .as_array()
        .expect("columns array present");

    for col in columns {
        assert!(
            col.get("compatible_hierarchies").is_none(),
            "column {:?} must not have compatible_hierarchies in raw-catalog mode",
            col.get("unique_name")
        );
    }
}

// ── AC3 ──────────────────────────────────────────────────────────────────────

#[test]
fn ac3_compatible_hierarchies_entries_have_correct_shape() {
    let srv = enriched_server();
    let result = call_tool(
        &srv,
        "describe_model",
        json!({ "model": "tpcds_benchmark_model" }),
    );

    let columns = result["structuredContent"]["columns"]
        .as_array()
        .expect("columns array");

    let mut checked = 0u32;
    for col in columns {
        if col.get("kind").and_then(Value::as_str) != Some("measure") {
            continue;
        }
        let Some(entries) = col.get("compatible_hierarchies").and_then(Value::as_array) else {
            continue;
        };
        for entry in entries {
            assert!(
                entry
                    .get("hierarchy_unique_name")
                    .and_then(Value::as_str)
                    .is_some(),
                "entry missing hierarchy_unique_name string: {entry}"
            );
            assert!(
                entry
                    .get("level_unique_names")
                    .and_then(Value::as_array)
                    .is_some(),
                "entry missing level_unique_names array: {entry}"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "at least one compatible_hierarchies entry shape-checked"
    );
}

// ── AC4 ──────────────────────────────────────────────────────────────────────

#[test]
fn ac4_query_response_always_has_filter_fields() {
    if !fleet_present() {
        eprintln!("AC4 SKIPPED: fleet binaries not present");
        return;
    }

    let srv = plain_server();
    let result = call_tool(
        &srv,
        "query_multidimensional",
        json!({ "mqo": sales_mqo(10) }),
    );

    let sc = result.get("structuredContent").expect("structuredContent present");
    assert!(
        sc.get("filters_applied").and_then(Value::as_array).is_some(),
        "filters_applied array absent: {sc}"
    );
    assert!(
        sc.get("filters_dropped").and_then(Value::as_array).is_some(),
        "filters_dropped array absent: {sc}"
    );
}

// ── AC5 ──────────────────────────────────────────────────────────────────────

#[test]
fn ac5_from_json_valid_enriched_returns_some() {
    // Minimal enriched JSON with one measure and one level (column_group populated).
    let enriched = json!({
        "columns": [
            {
                "unique_name": "sales.Revenue",
                "label": "Revenue",
                "kind": "measure",
                "is_calc": false,
                "column_group": ["sales"]
            },
            {
                "unique_name": "date.[Year]",
                "label": "Year",
                "kind": "level",
                "hierarchy": "date",
                "level": "Year",
                "is_calc": false,
                "column_group": ["date"]
            }
        ]
    });

    let result = ServerEnrichedData::from_json(&enriched);
    assert!(
        result.is_some(),
        "from_json must return Some for valid enriched JSON"
    );
}

// ── AC6 ──────────────────────────────────────────────────────────────────────

#[test]
fn ac6_from_json_missing_columns_returns_none() {
    let bad = json!({ "model": "sales", "schema": "test" });
    let result = ServerEnrichedData::from_json(&bad);
    assert!(
        result.is_none(),
        "from_json must return None when columns key is absent"
    );
}

// ── AC7 ──────────────────────────────────────────────────────────────────────

#[test]
fn ac7_auto_derivation_from_tpcds_produces_nonempty_map() {
    let enriched = tpcds_enriched();

    assert!(
        !enriched.compatible_hierarchies.is_empty(),
        "compatible_hierarchies map must be non-empty for tpcds catalog"
    );

    // Every entry must have at least one compatible hierarchy.
    for (measure_un, compat) in &enriched.compatible_hierarchies {
        let arr = compat.as_array().expect("value is array");
        assert!(
            !arr.is_empty(),
            "measure {measure_un} has zero compatible hierarchies"
        );
    }
}

// ── AC8 ──────────────────────────────────────────────────────────────────────

#[test]
fn ac8_catalog_json_round_trips() {
    let enriched = tpcds_enriched();

    let reparsed: Value = serde_json::from_str(&enriched.catalog_json)
        .expect("catalog_json must be valid JSON");

    assert!(
        reparsed.get("columns").and_then(Value::as_array).is_some(),
        "round-tripped catalog_json must contain 'columns' array"
    );
}

// ── AC9 ──────────────────────────────────────────────────────────────────────

#[test]
fn ac9_per_measure_compatible_hierarchies_count_matches_enriched_measures() {
    let catalog = load_tpcds_catalog();
    let snap: CatalogSnapshot =
        serde_json::from_value(catalog).expect("parse CatalogSnapshot");
    let bindings = FactBindings::tpcds_defaults();
    let enriched_cat = enrich(&snap, &bindings);
    let enriched_json = serde_json::to_value(&enriched_cat).expect("serialize enriched");
    let enriched = ServerEnrichedData::from_json(&enriched_json)
        .expect("build ServerEnrichedData from enriched tpcds");

    // Count measures in the enriched catalog JSON.
    let measure_count = enriched_json
        .get("columns")
        .and_then(Value::as_array)
        .map(|cols| {
            cols.iter()
                .filter(|c| c.get("kind").and_then(Value::as_str) == Some("measure"))
                .count()
        })
        .unwrap_or(0);

    assert!(measure_count > 0, "enriched catalog must have measures");
    assert_eq!(
        enriched.compatible_hierarchies.len(),
        measure_count,
        "compatible_hierarchies map must have one entry per measure"
    );
}

// ── AC10 ─────────────────────────────────────────────────────────────────────

#[test]
fn ac10_cross_fact_incompatible_variant_report_shape() {
    let report = json!({
        "incompatible": [
            {
                "measure_unique_name": "sales.Revenue",
                "dimension_unique_name": "store_dimension",
                "reason": "different fact tables"
            }
        ]
    });
    let err = PipelineError::CrossFactIncompatible {
        report: report.clone(),
    };

    match &err {
        PipelineError::CrossFactIncompatible { report: r } => {
            let arr = r
                .get("incompatible")
                .and_then(Value::as_array)
                .expect("report must have 'incompatible' array");
            assert!(!arr.is_empty(), "incompatible array must be non-empty");
            let first = &arr[0];
            assert!(
                first.get("measure_unique_name").and_then(Value::as_str).is_some(),
                "first entry must have measure_unique_name"
            );
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}
