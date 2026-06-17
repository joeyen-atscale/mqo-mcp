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
        inline_threshold: mqo_mcp_server::INLINE_THRESHOLD,
        enriched: Some(Arc::new(tpcds_enriched())),
        xmla_model_coords: HashMap::new(),
        max_projection_cardinality: mqo_mcp_server::DEFAULT_MAX_PROJECTION_CARDINALITY,
        model_graph: None,
            autolift_base_url: None,
            autolift_cache: None,
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
        inline_threshold: mqo_mcp_server::INLINE_THRESHOLD,
        enriched: None,
        xmla_model_coords: HashMap::new(),
        max_projection_cardinality: mqo_mcp_server::DEFAULT_MAX_PROJECTION_CARDINALITY,
        model_graph: None,
            autolift_base_url: None,
            autolift_cache: None,
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

// ── Param-validator wiring (Item 2) ──────────────────────────────────────────

/// The pre-execution param-validator fires on a grounded-but-wrong hierarchy
/// level: `time.calendar` exists and has levels Year/Month, but `Quarter` is
/// not one of them. The validator must reject with `param_rejected` BEFORE any
/// subprocess runs — no compiled query, no execution.
#[test]
fn param_validator_rejects_wrong_hierarchy_level_pre_execution() {
    if !fleet_present() {
        eprintln!("param-validator wrong-level SKIPPED: fleet binaries not present");
        return;
    }
    let srv = plain_server();
    let mqo = json!({
        "model": "sales",
        "measures": [{ "unique_name": "Revenue" }],
        // time.calendar is a real hierarchy, but "Quarter" is not one of its
        // levels (Year, Month) — WrongHierarchyLevel, not Unmapped.
        "dimensions": [{ "hierarchy": "time.calendar", "level": "Quarter" }],
        "filters": [],
        "time_intelligence": [],
        "order": null,
        "limit": 10,
        "non_empty": true
    });
    let result = call_tool(&srv, "query_multidimensional", json!({ "mqo": mqo }));
    assert_eq!(result["isError"], json!(true), "wrong level → error: {result}");
    let err = &result["structuredContent"]["error"];
    assert_eq!(
        err["code"],
        json!("param_rejected"),
        "validator surfaces param_rejected before binding: {err}"
    );
    // No execution happened.
    assert!(
        result["structuredContent"].get("compiled_query").is_none(),
        "no compiled query for a param-rejected MQO"
    );
    // The report names the offending level and carries a nearest-match suggestion.
    let detail = err["detail"].to_string();
    assert!(detail.contains("Quarter"), "report names the bad level: {detail}");
}

/// A fully-valid query against the sales catalog must NOT be rejected by the
/// param-validator (zero false positives): the label-referenced measure
/// "Revenue" (→ sales.revenue) and a real level resolve cleanly, so the query
/// proceeds to execution and returns rows.
#[test]
fn param_validator_passes_valid_query_unchanged() {
    if !fleet_present() {
        eprintln!("param-validator happy-path SKIPPED: fleet binaries not present");
        return;
    }
    let srv = plain_server();
    let mqo = json!({
        "model": "sales",
        "measures": [{ "unique_name": "Revenue" }],
        "dimensions": [{ "hierarchy": "time.calendar", "level": "Year" }],
        "filters": [],
        "time_intelligence": [],
        "order": null,
        "limit": 10,
        "non_empty": true
    });
    let result = call_tool(&srv, "query_multidimensional", json!({ "mqo": mqo }));
    assert_ne!(
        result["isError"],
        json!(true),
        "valid query must not be param-rejected: {result}"
    );
    assert!(
        result["structuredContent"].get("compiled_query").is_some(),
        "valid query executes and produces a compiled query: {result}"
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

// ── Disambiguation pack (PRD-mqo-describe-disambiguation-pack) ───────────────

/// Helper: describe_model over the full tpcds catalog. No `model` filter — in
/// the tpcds fixture only measures carry the `tpcds_benchmark_model.` prefix;
/// dimension levels use hierarchy-prefixed unique_names, so the unfiltered call
/// is the realistic shape that includes both measures and levels.
fn tpcds_describe() -> Value {
    let srv = enriched_server();
    call_tool(&srv, "describe_model", json!({}))
}

/// AC-1: every dimension level carries `hierarchy` + `level`.
#[test]
fn disambig_ac1_levels_have_hierarchy_and_level() {
    let result = tpcds_describe();
    let columns = result["structuredContent"]["columns"]
        .as_array()
        .expect("columns");
    let levels: Vec<&Value> = columns
        .iter()
        .filter(|c| c.get("kind").and_then(Value::as_str) == Some("level"))
        .collect();
    assert!(!levels.is_empty(), "tpcds has dimension levels");
    for l in &levels {
        let un = l["unique_name"].as_str().unwrap_or("?");
        assert!(
            l.get("hierarchy").and_then(Value::as_str).is_some(),
            "level {un} missing hierarchy"
        );
        assert!(
            l.get("level").and_then(Value::as_str).is_some(),
            "level {un} missing level"
        );
    }
}

/// AC-2: describe_model emits a top-level `near_twins` block for the known
/// TPC-DS conflicts (Brand Name across 3 product hierarchies, State Name
/// across 6 address/store/warehouse hierarchies).
#[test]
fn disambig_ac2_near_twins_present_for_known_conflicts() {
    let result = tpcds_describe();
    let groups = result["structuredContent"]["near_twins"]
        .as_array()
        .expect("near_twins block present");
    assert!(!groups.is_empty(), "tpcds has near-twin groups");

    // Find "brand name" and "state name" groups.
    let find = |core: &str| -> Option<&Value> {
        groups
            .iter()
            .find(|g| g.get("core_label").and_then(Value::as_str) == Some(core))
    };

    let brand = find("brand name").expect("brand name near-twin group");
    let brand_twins = brand["near_twins"].as_array().unwrap();
    let brand_uns: Vec<&str> = brand_twins
        .iter()
        .map(|t| t["unique_name"].as_str().unwrap())
        .collect();
    assert!(
        brand_uns.contains(&"product_dimension.[Product Brand Name]"),
        "canonical Product Brand Name present: {brand_uns:?}"
    );
    assert!(
        brand_uns.contains(&"store_item_product_dimension.[Store Item Product Brand Name]"),
        "Store Item variant present: {brand_uns:?}"
    );

    // AC-3 surrogate: the canonical_for hint points to Product Brand Name
    // (fm2-002 wanted Product Brand Name, not the Store Item near-twin).
    let canonical = brand_twins
        .iter()
        .find(|t| t.get("canonical_for").is_some())
        .expect("a canonical twin");
    assert_eq!(
        canonical["unique_name"].as_str().unwrap(),
        "product_dimension.[Product Brand Name]",
        "Product Brand Name is canonical for generic 'brand' questions"
    );

    let state = find("state name").expect("state name near-twin group");
    assert!(
        state["near_twins"].as_array().unwrap().len() >= 2,
        "state name spans multiple hierarchies"
    );
}

/// Wire-grounding gate: describe_model called WITH a `model` filter (the
/// realistic shape — measures live under `tpcds_benchmark_model.`, so a
/// model-scoped call previously dropped every dimension level and produced an
/// empty `near_twins`). The level-twin pass must read levels from the full
/// catalog, and the measure-twin pass must surface lookalike measures. Asserts
/// `near_twins` is non-empty and contains BOTH a level-twin group (Brand Name
/// across product hierarchies) AND a measure-twin group (e.g. "sales price"
/// across Catalog/Store/Web fact-group prefixes).
#[test]
fn wire_grounding_model_filtered_describe_yields_level_and_measure_twins() {
    let srv = enriched_server();
    // Model-scoped call: only `tpcds_benchmark_model.*` measures pass the column
    // filter — levels are dropped from `columns`, exactly the v0.14.0 blocker.
    let result = call_tool(
        &srv,
        "describe_model",
        json!({ "model": "tpcds_benchmark_model" }),
    );
    let groups = result["structuredContent"]["near_twins"]
        .as_array()
        .expect("near_twins block present")
        .clone();
    assert!(
        !groups.is_empty(),
        "model-filtered describe_model must still populate near_twins from the full catalog"
    );

    let level_groups: Vec<&Value> = groups
        .iter()
        .filter(|g| g.get("twin_kind").and_then(Value::as_str) == Some("level"))
        .collect();
    let measure_groups: Vec<&Value> = groups
        .iter()
        .filter(|g| g.get("twin_kind").and_then(Value::as_str) == Some("measure"))
        .collect();

    assert!(
        !level_groups.is_empty(),
        "near_twins must include at least one dimension-level twin group (wrong_hierarchy_level)"
    );
    assert!(
        !measure_groups.is_empty(),
        "near_twins must include at least one measure twin group (lookalike_measure)"
    );

    // A concrete level twin: Brand Name spans ≥2 product hierarchies.
    let brand = level_groups
        .iter()
        .find(|g| g.get("core_label").and_then(Value::as_str) == Some("brand name"))
        .expect("brand name level-twin group");
    assert!(
        brand["near_twins"].as_array().unwrap().len() >= 2,
        "brand name level twin spans multiple hierarchies"
    );

    // At least one measure twin group must carry `measure_group` prefixes and
    // span ≥2 distinct prefixes (e.g. catalog/store/web variants of one concept).
    // The footprint guard may trim smaller groups first, so we search across all
    // surviving groups rather than assuming the first one is multi-prefix.
    let empty_vec: Vec<Value> = vec![];
    let multi_prefix_group = measure_groups.iter().find(|g| {
        let prefixes: std::collections::BTreeSet<&str> = g["near_twins"]
            .as_array()
            .unwrap_or(&empty_vec)
            .iter()
            .filter_map(|m| m.get("measure_group").and_then(Value::as_str))
            .collect();
        prefixes.len() >= 2
    });
    assert!(
        multi_prefix_group.is_some(),
        "at least one measure twin group must span ≥2 fact-group prefixes; groups: {measure_groups:?}"
    );
}

/// AC: each measure carries a `date_roles` array (may be empty, never absent).
#[test]
fn disambig_measures_carry_date_roles() {
    let result = tpcds_describe();
    let columns = result["structuredContent"]["columns"]
        .as_array()
        .expect("columns");
    let measures: Vec<&Value> = columns
        .iter()
        .filter(|c| c.get("kind").and_then(Value::as_str) == Some("measure"))
        .collect();
    assert!(!measures.is_empty(), "tpcds has measures");
    for m in &measures {
        let un = m["unique_name"].as_str().unwrap_or("?");
        let roles = m.get("date_roles");
        assert!(roles.is_some(), "measure {un} missing date_roles");
        assert!(
            roles.unwrap().is_array(),
            "measure {un} date_roles must be an array"
        );
    }
    // TPC-DS has date hierarchies, so the array should be non-empty.
    let first = &measures[0];
    assert!(
        !first["date_roles"].as_array().unwrap().is_empty(),
        "tpcds measures should carry derived date_roles"
    );
}

/// AC-5: the enriched describe_model adds ≤15% byte footprint vs the
/// pre-disambiguation response (columns + compatible_hierarchies, no new keys).
#[test]
fn disambig_ac5_footprint_within_15pct() {
    let result = tpcds_describe();
    let columns = result["structuredContent"]["columns"]
        .as_array()
        .expect("columns")
        .clone();
    let near_twins = result["structuredContent"]["near_twins"].clone();

    // Original = columns WITHOUT the disambiguation-added fields, and without
    // the near_twins block. We strip the additive fields to reconstruct the
    // pre-feature baseline.
    let stripped: Vec<Value> = columns
        .iter()
        .map(|c| {
            let mut c = c.clone();
            if let Some(obj) = c.as_object_mut() {
                obj.remove("date_roles");
                // hierarchy/level are pre-existing in the snapshot; near_twins
                // is the only structurally-new top-level block. We count the
                // new top-level block + date_roles as the added bytes.
            }
            c
        })
        .collect();

    let baseline = serde_json::to_string(&json!({ "columns": stripped }))
        .unwrap()
        .len();
    let enriched = serde_json::to_string(&json!({
        "columns": columns,
        "near_twins": near_twins,
    }))
    .unwrap()
    .len();

    let overhead = (enriched as f64 - baseline as f64) / baseline as f64;
    assert!(
        overhead <= 0.15,
        "describe_model footprint grew {:.1}% (> 15%): baseline={baseline} enriched={enriched}",
        overhead * 100.0
    );
}
