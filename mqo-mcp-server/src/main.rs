//! `mqo-mcp-server` — MCP server exposing `query_multidimensional` and three
//! read-only catalog tools over JSON-RPC 2.0 on stdio.
//!
//! Usage (fixture mode — default, no cluster required):
//!
//! ```text
//! mqo-mcp-server --catalog <snapshot.json> [--stats <stats.json>]
//!                [--release-dir <dir>] [--row-threshold <N>]
//! ```
//!
//! Usage (live mode — connects to an `AtScale` endpoint):
//!
//! ```text
//! mqo-mcp-server --catalog <snapshot.json> \
//!                --endpoint <host:port> [--xmla-url <url>] \
//!                --oidc-token-url <url> --oidc-client-id <id> \
//!                --oidc-realm <realm> --oidc-client-secret-env <VARNAME>
//! ```
//!
//! DAX is the primary backend. On the reference cluster
//! (`mcp-aws.atscaleinternal.com`) the XMLA URL is derived automatically as
//! `https://<endpoint-host>/v1/xmla` when `--xmla-url` is omitted, so DAX/MDX
//! reach the engine without the operator memorizing the path. SQL (`PGWire`)
//! remains available as an explicit, operator-selectable fallback via
//! `--force-backend sql`.
//!
//! The server reads newline-delimited JSON-RPC requests on stdin and writes
//! newline-delimited responses on stdout (the MCP stdio transport).

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    // Pre-existing lint suppressions — do not remove without fixing the underlying code.
    clippy::doc_markdown,
    clippy::if_not_else,
    clippy::single_match_else,
    clippy::struct_excessive_bools,
    clippy::too_many_lines,
    clippy::uninlined_format_args,
)]

use clap::Parser;
use mcp_cluster_health_monitor::report::OverallStatus;
use mcp_cluster_registry::ClusterRegistry;
use mqo_mcp_server::{
    autolift::AutoliftCache,
    catalog_cache::{
        apply_cached_columns, cardinality_map, default_cache_path, fetch_schema_update,
        ingest_cardinalities_only, load_cache, save_cache, validate_cache, CacheVerdict,
        CatalogCache, CACHE_FORMAT_VERSION,
    },
    cursor::{CursorStore, DEFAULT_CURSOR_TTL_SECS, DEFAULT_PAGE_SIZE},
    mcp::{discover_xmla_coords, DEFAULT_MAX_PROJECTION_CARDINALITY},
    run_health_check_sync, BackendCapabilities, EndpointConfig, LiveExecutor, OidcConfig, Server,
    ServerEnrichedData, ServerEngine, ToolPaths,
};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_ROW_THRESHOLD: u64 = 50_000;

#[derive(Parser, Debug)]
#[command(
    name = "mqo-mcp-server",
    about = "MCP server: query_multidimensional (bind→route→compile→execute) + \
             read-only catalog tools. Without --endpoint uses the fixture engine \
             (cluster-free default). With --endpoint + OIDC flags, connects to a \
             live AtScale endpoint."
)]
struct Args {
    /// Path to the recorded catalog snapshot JSON.
    #[arg(long)]
    catalog: PathBuf,

    /// Path to the router stats JSON. Defaults to an empty stats bundle.
    #[arg(long)]
    stats: Option<PathBuf>,

    /// Directory containing the fleet release binaries.
    #[arg(long)]
    release_dir: Option<PathBuf>,

    /// Router row threshold above which the SQL extract path is chosen.
    #[arg(long, default_value_t = DEFAULT_ROW_THRESHOLD)]
    row_threshold: u64,

    /// Per-handle **materialization budget**: the maximum number of rows the
    /// server fetches and persists into a handle (PRD-mqo-handle-full-
    /// materialization). The persisted handle — and every `dataset_*` op /
    /// `dataset_export` over it — holds the full result up to this budget,
    /// instead of the old hard-coded 1000-row clamp.
    ///
    /// This is decoupled from the inline-sample bound (`--inline-threshold`):
    /// raising it does NOT enlarge what `query_multidimensional` puts in the
    /// LLM context. When the real result exceeds the budget, the response
    /// carries a typed `result_too_large` over-budget signal (never a silent
    /// clamp presented as complete). Clamped to 1..=200000 (the upstream PGWire
    /// ceiling). Set to 1000 to reproduce the pre-fix behavior exactly.
    #[arg(long, default_value_t = mqo_mcp_server::DEFAULT_MAX_RESULT_ROWS)]
    max_result_rows: usize,

    /// Live-mode only: at startup, probe the cluster for each dimension level's
    /// enumerated member domain (one bounded query per level) and layer
    /// value_type/domain onto the in-memory catalog — the live source for the
    /// validator filter-level guard and the binder member-grounding check
    /// (PRD-mqo-live-catalog-ingestion). Off by default (opt-in); ignored in
    /// fixture mode. Fail-open: per-level probe errors are skipped, never fatal.
    #[arg(long)]
    capture_live_domains: bool,

    /// Max distinct members enumerated per level (domains above this carry a
    /// descriptor only). Bounds snapshot growth + per-level cost.
    #[arg(long, default_value_t = 1000)]
    catalog_domain_cap: usize,

    /// Max number of levels probed during live domain ingestion (bounds startup
    /// wall-time on wide models). Default is effectively unlimited — all levels
    /// with cardinality ≤ domain_cap are captured; only high-cardinality levels
    /// are skipped. Set to a small value to cap wall-time on unusually wide models.
    #[arg(long, default_value_t = usize::MAX)]
    catalog_max_levels: usize,

    /// Max number of MDSCHEMA_MEMBERS Discover requests in flight simultaneously
    /// during live domain ingestion. Default 16 gives ~6× speedup on typical
    /// models (157 levels × ~1.1 s serial → target < 30 s). Set to 1 to
    /// reproduce the old serial path exactly (regression floor / rate-limit mode).
    #[arg(long, default_value_t = 16)]
    catalog_ingest_concurrency: usize,

    /// Model (cube) name the domain probe queries against. Defaults to the sole
    /// discovered XMLA model when there is exactly one; required when the cluster
    /// exposes multiple cubes (otherwise probe queries bind to an arbitrary model
    /// and capture nothing).
    #[arg(long, value_name = "MODEL")]
    catalog_model: Option<String>,

    /// `AtScale` `PGWire` endpoint as `<host:port>` (e.g. `localhost:15432`).
    /// Presence selects live mode; absence (default) selects fixture mode.
    #[arg(long, value_name = "HOST:PORT")]
    endpoint: Option<String>,

    /// `AtScale` HTTP XMLA engine URL for the primary DAX path (also serves MDX).
    /// Example: `https://mcp-aws.atscaleinternal.com/v1/xmla`.
    ///
    /// When omitted, the URL is derived from the `--endpoint` host as
    /// `https://<host>/v1/xmla` (the engine's HTTP XMLA path). If it cannot be
    /// derived and a DAX/MDX backend is selected, the query fails with a
    /// structured error naming `/v1/xmla` — the server never POSTs to an empty
    /// URL. Note: `/engine/xmla/`, `/xmla`, and `/dax` route to the Modeler app,
    /// not the engine; use `/v1/xmla`.
    #[arg(long, value_name = "URL")]
    xmla_url: Option<String>,

    /// OIDC token endpoint URL.
    #[arg(long, value_name = "URL")]
    oidc_token_url: Option<String>,

    /// OIDC client ID.
    #[arg(long, value_name = "ID")]
    oidc_client_id: Option<String>,

    /// Keycloak realm name.
    #[arg(long, value_name = "REALM")]
    oidc_realm: Option<String>,

    /// Name of the environment variable that holds the OIDC client secret.
    /// The secret is read from the named env var at startup and never passed
    /// as a flag value; it will not appear in logs or `--help` output.
    #[arg(long, value_name = "VARNAME")]
    oidc_client_secret_env: Option<String>,

    /// OIDC ROPC username. When set, the XMLA token fetch uses
    /// `grant_type=password` (Resource Owner Password Credentials) instead of
    /// `client_credentials`. Must be paired with --oidc-password-env. This is the
    /// non-secret username value, not an env-var name.
    #[arg(long, value_name = "USER")]
    oidc_username: Option<String>,

    /// Name of the environment variable that holds the OIDC ROPC user password.
    /// Only used when --oidc-username is set. The secret is read from the named
    /// env var; it is never accepted as a flag value and never logged.
    #[arg(long, value_name = "VARNAME")]
    oidc_password_env: Option<String>,

    /// `PGWire` username override. When set, the PGWire (SQL) path uses direct
    /// credentials instead of an OIDC bearer token. This affects SQL auth ONLY:
    /// XMLA (DAX/MDX) still uses the OIDC token provider when the --oidc-* flags
    /// are present. Must be paired with --pg-pass-env.
    #[arg(long, value_name = "USER")]
    pg_user: Option<String>,

    /// Name of the environment variable that holds the `PGWire` password when
    /// using direct-credential auth (--pg-user). Never accepted as a flag value.
    #[arg(long, value_name = "VARNAME")]
    pg_pass_env: Option<String>,

    /// Override the router's backend selection for every query.
    /// Accepted values: `dax`, `mdx`, `sql`.
    ///
    /// DAX is the primary backend and the router's default choice for
    /// aggregated queries; it reaches the engine over `--xmla-url` (`/v1/xmla`).
    /// `--force-backend sql` is an explicit fallback that pins the `PGWire` SQL
    /// path — it is NOT required on mcp-aws, where DAX over `/v1/xmla` works.
    #[arg(long, value_name = "BACKEND")]
    force_backend: Option<String>,

    /// Path to a cluster registry TOML (enables federation mode).
    /// When absent, the server behaves exactly as single-cluster mode.
    #[arg(long, value_name = "REGISTRY_TOML")]
    registry: Option<PathBuf>,

    /// Run a health check against all registered clusters at startup.
    /// Exits with code 1 if any `required: true` cluster is unhealthy.
    /// Only meaningful when `--registry` is also set.
    #[arg(long, default_value_t = false)]
    health_check: bool,

    /// Skip backend capability probe at startup (probe runs by default in live mode).
    /// When set, all backends are assumed live and no downgrade logic is applied.
    #[arg(long, default_value_t = false)]
    no_probe: bool,

    /// Number of rows per page in cursor mode. When a query result exceeds this
    /// threshold, the server persists the full result and returns a first page
    /// plus a `cursor_id`. Queries at or below this threshold are returned inline
    /// (backward-compatible v0.3.0 behavior).
    #[arg(long, default_value_t = DEFAULT_PAGE_SIZE)]
    page_size: usize,

    /// Inline-row threshold (K). `query_multidimensional` and every `dataset_*`
    /// op inline raw `rows` only when the result's `row_count` is at or below
    /// this value. Above K the response carries a bounded summary + a handle and
    /// NO `rows` — the structural anti-calculator guarantee. Default: 25.
    #[arg(long, default_value_t = mqo_mcp_server::INLINE_THRESHOLD)]
    inline_threshold: usize,

    /// Path to a pre-derived `enriched-catalog.v1` JSON file (from
    /// `mqoguard-column-group-enrichment`). When provided, skip auto-derivation
    /// and load this file directly. When absent, the server attempts to derive the
    /// enriched catalog from `--catalog` using the library API. Enrichment failure
    /// is a warning, not a startup error (raw-catalog fallback).
    #[arg(long)]
    enriched_catalog: Option<PathBuf>,

    /// Cursor TTL in seconds. Cursors that have not been accessed within this
    /// window are evicted from the store. Default: 600 (10 minutes).
    #[arg(long, default_value_t = DEFAULT_CURSOR_TTL_SECS)]
    cursor_ttl_secs: u64,

    /// Maximum allowed distinct-row cardinality estimate for a projection MQO.
    ///
    /// Before executing a measureless (projection) MQO, the server estimates the
    /// distinct-row count from catalog level member counts and filter selectivity.
    /// When the estimate exceeds this value, the server returns a typed
    /// `projection_too_large` decline — no execution, no credits spent.
    ///
    /// Default: 10,000 (well below the engine row cap of ~50,000 so the guard
    /// always fires before the engine would cap-and-spend).  Set to 0 to always
    /// decline all projection queries.
    #[arg(long, default_value_t = DEFAULT_MAX_PROJECTION_CARDINALITY)]
    max_projection_cardinality: usize,

    /// Path to a static XMLA catalog-map JSON file.
    ///
    /// Maps MQO model/cube names to their XMLA catalog and cube coordinates.
    /// Required when XMLA discovery is unavailable or unreliable; also used as
    /// an override when provided alongside `--xmla-url`.
    ///
    /// Format: `{"<cube_name>": {"catalog": "<xmla_catalog>", "cube": "<cube_name>"}, …}`
    ///
    /// Example:
    /// ```json
    /// {"tpcds_benchmark_model": {"catalog": "tpcds_Snowflake", "cube": "tpcds_benchmark_model"}}
    /// ```
    ///
    /// When absent in live mode and `--xmla-url` is set, the server attempts
    /// `DBSCHEMA_CATALOGS` + `MDSCHEMA_CUBES` discovery at startup and logs the
    /// result. If discovery also fails, DAX/MDX queries fail with a structured
    /// `xmla_coords_not_found` error naming the missing model.
    #[arg(long, value_name = "PATH")]
    xmla_catalog_map: Option<PathBuf>,

    /// Path for the enriched-catalog disk cache (PRD-mqo-catalog-disk-cache).
    /// Defaults to `<catalog>.enriched-cache.json` when `--capture-live-domains`
    /// is set. The cache persists the enriched catalog (domains, cardinalities,
    /// value_types, semi-additive flags) and is validated cheaply on restart via
    /// `LAST_SCHEMA_UPDATE` + per-level cardinality diff — no per-level member
    /// fetches unless something actually changed.
    #[arg(long, value_name = "PATH")]
    catalog_cache_path: Option<PathBuf>,

    /// Catalog cache TTL in seconds.  When the cache is older than this value a
    /// full re-ingest is forced regardless of schema/cardinality signals.
    /// Default: 86400 (24 hours).
    #[arg(long, default_value_t = 86_400_u64)]
    catalog_cache_ttl: u64,

    /// Ignore the on-disk catalog cache and force a full live ingest.  The new
    /// result overwrites the cache file.  Use after a data load that the
    /// `LAST_SCHEMA_UPDATE` + cardinality signals cannot detect.
    #[arg(long, default_value_t = false)]
    refresh_catalog: bool,

    /// Base URL for the engine catalog-XML REST endpoint used by the OSL
    /// auto-lift tier (PRD-osl-live-autolift).
    ///
    /// When set, `query_model_graph` fetches `{base_url}/{catalog_id}.xml`
    /// with an OIDC bearer token, lifts the XML into the in-process RDF triple
    /// store, and caches the graph keyed on `(catalog_id, LAST_SCHEMA_UPDATE)`.
    /// On cache hit (same schema version) the stored graph is served without
    /// re-fetching.
    ///
    /// The global mount prefix (e.g. `/api/1.0`) is included in the URL when
    /// present: `https://<host>/api/1.0/catalogs/{catalogId}.xml`.
    ///
    /// Example: `https://mcp-aws.atscaleinternal.com/api/1.0`
    ///
    /// When absent (the default), auto-lift is disabled and `query_model_graph`
    /// returns `model_graph_not_available` for all live models.
    ///
    /// Can also be set via the `ATSCALE_CATALOG_XML_BASE` environment variable.
    #[arg(long, env = "ATSCALE_CATALOG_XML_BASE", value_name = "URL")]
    autolift_base_url: Option<String>,
}

fn main() {
    let args = Args::parse();

    let mut catalog = load_json(&args.catalog);
    let stats = args.stats.as_deref().map_or_else(
        || serde_json::json!({ "level_cardinalities": {}, "shape_flags": {} }),
        load_json,
    );

    let tools = ToolPaths::resolve(args.release_dir.as_deref());
    let engine = build_engine(&args);

    // ── Federation registry (optional) ────────────────────────────────────
    let (registry, health_cache) = if let Some(ref reg_path) = args.registry {
        let toml_text = std::fs::read_to_string(reg_path).unwrap_or_else(|e| {
            eprintln!("mqo-mcp-server: cannot read registry {}: {e}", reg_path.display());
            process::exit(2);
        });
        let reg = ClusterRegistry::from_toml(&toml_text).unwrap_or_else(|e| {
            eprintln!("mqo-mcp-server: invalid registry {}: {e}", reg_path.display());
            process::exit(2);
        });
        eprintln!(
            "mqo-mcp-server: federation mode: {} cluster(s) loaded from {}",
            reg.clusters.len(),
            reg_path.display()
        );

        let cache: Arc<Mutex<Option<mcp_cluster_health_monitor::report::HealthReport>>> =
            Arc::new(Mutex::new(None));

        // Optional startup health check.
        if args.health_check {
            eprintln!("mqo-mcp-server: running startup health check…");
            let report = run_health_check_sync(&reg, 5000);
            let overall = &report.overall;
            eprintln!("mqo-mcp-server: health check overall: {overall}");

            if matches!(overall, OverallStatus::Critical) {
                eprintln!("mqo-mcp-server: critical cluster(s) unhealthy — aborting (--health-check)");
                process::exit(1);
            }

            // Seed the cache.
            if let Ok(mut guard) = cache.lock() {
                *guard = Some(report);
            }
        }

        (Some(Arc::new(reg)), Some(cache))
    } else {
        (None, None)
    };

    // ── XMLA model coordinate map ─────────────────────────────────────────
    // Must run BEFORE the capability probe so that the probe can resolve bare
    // model names (e.g. `tpcds_benchmark_model`) to 3-segment XMLA paths
    // (e.g. `atscale_catalogs.tpcds_Snowflake.tpcds_benchmark_model`).
    // In live mode: load static map first (takes priority), then auto-discover
    // from the XMLA endpoint when no static map is provided. In fixture mode:
    // the map is empty (fixture engine does not use it for routing).
    let xmla_model_coords = build_xmla_model_coords(&args, &engine);

    // ── Backend capability probe (live mode only, unless --no-probe) ──────
    let capabilities = match &engine {
        ServerEngine::Fixture => {
            // Fixture mode: no cluster, all backends reported as live.
            BackendCapabilities::all_live()
        }
        ServerEngine::Live(ex) => {
            if args.no_probe {
                eprintln!("mqo-mcp-server: backend probe: skipped (--no-probe)");
                BackendCapabilities::all_live()
            } else {
                let caps = BackendCapabilities::probe(ex.as_ref(), Some(&catalog), &xmla_model_coords);
                eprintln!(
                    "mqo-mcp-server: backends: dax={} mdx={} sql={}",
                    caps.dax, caps.mdx, caps.sql
                );
                caps
            }
        }
    };

    let cursor_store = Arc::new(CursorStore::new(args.cursor_ttl_secs));
    eprintln!(
        "mqo-mcp-server: cursor: page_size={} ttl={}s",
        args.page_size, args.cursor_ttl_secs
    );

    // ── Live catalog domain ingestion (opt-in, live mode only) ────────────
    // PRD-mqo-live-catalog-ingestion: probe the cluster for level member
    // domains and layer them onto the catalog so the filter-level + member-
    // grounding guards run on live data instead of a hand-edited snapshot.
    //
    // PRD-mqo-catalog-disk-cache: wrap the unconditional ingest with a
    // validity gate.  On startup:
    //   1. Try to load the on-disk cache (unless --refresh-catalog).
    //   2. If a cache is present, run the cheap validation Discovers
    //      (MDSCHEMA_CUBES + MDSCHEMA_LEVELS — no per-level MEMBERS fetches).
    //   3. Verdict:
    //      Valid           → apply cached columns, skip ingest.
    //      PartialInvalid  → apply cached columns, re-fetch only changed levels.
    //      FullReingest    → run ingest_live_metadata as before.
    //   4. After any ingest (full or partial): write the cache.
    // Cluster unreachable during validation → serve cache with a warning log.
    if args.capture_live_domains {
        match &engine {
            ServerEngine::Live(ex) => {
                // Prefer the explicit --catalog-model; else the sole discovered
                // model; else the first key with a warning.
                let model = args.catalog_model.clone().or_else(|| {
                    if xmla_model_coords.len() > 1 {
                        eprintln!(
                            "mqo-mcp-server: WARN: --capture-live-domains: {} models discovered \
                             and no --catalog-model given; using an arbitrary one",
                            xmla_model_coords.len()
                        );
                    }
                    xmla_model_coords.keys().next().cloned()
                });
                // Resolve the XMLA (catalog, cube) for the chosen model.
                let coords = model.as_ref().and_then(|m| xmla_model_coords.get(m).cloned());
                match coords {
                    Some((xmla_catalog, cube)) => {
                        let cfg = mqo_mcp_server::catalog_ingest::IngestConfig {
                            domain_cap: args.catalog_domain_cap,
                            max_levels: args.catalog_max_levels,
                            concurrency: args.catalog_ingest_concurrency,
                        };

                        // ── Resolve cache path ────────────────────────────
                        let cache_path = args
                            .catalog_cache_path
                            .clone()
                            .unwrap_or_else(|| default_cache_path(&args.catalog));

                        // ── Current wall-clock (Unix seconds) ────────────
                        let now_secs = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);

                        // ── Try cache path ────────────────────────────────
                        let existing_cache: Option<CatalogCache> = if args.refresh_catalog {
                            eprintln!("mqo-mcp-server: catalog cache: --refresh-catalog set — ignoring cache");
                            None
                        } else {
                            load_cache(&cache_path)
                        };

                        let mut did_full_ingest = false;

                        if let Some(ref cached) = existing_cache {
                            // ── Cheap validation: MDSCHEMA_CUBES + MDSCHEMA_LEVELS ──
                            // If cluster is unreachable here, serve the cache with a warning.
                            let validation_ok = ex.discover_mdschema("MDSCHEMA_CUBES", &xmla_catalog, &cube, None).is_ok();
                            if !validation_ok {
                                let age = now_secs.saturating_sub(cached.captured_at);
                                eprintln!(
                                    "mqo-mcp-server: WARN: cluster unreachable for cache \
                                     validation — serving cached domains (age: {age}s)"
                                );
                                apply_cached_columns(&mut catalog, &cached.columns);
                            } else {
                                let fresh_schema_update =
                                    fetch_schema_update(ex, &xmla_catalog, &cube);
                                let fresh_cardinalities =
                                    ingest_cardinalities_only(ex, &xmla_catalog, &cube, &catalog);
                                let cache_cardinalities = cardinality_map(&cached.columns);

                                let verdict = validate_cache(
                                    cached,
                                    fresh_schema_update.as_deref(),
                                    &fresh_cardinalities,
                                    &cache_cardinalities,
                                    args.catalog_cache_ttl,
                                    now_secs,
                                );

                                match verdict {
                                    CacheVerdict::Valid => {
                                        eprintln!(
                                            "mqo-mcp-server: catalog cache: serving cached \
                                             domains (validated)"
                                        );
                                        apply_cached_columns(&mut catalog, &cached.columns);
                                    }
                                    CacheVerdict::PartialInvalid(ref changed_luns) => {
                                        eprintln!(
                                            "mqo-mcp-server: catalog cache: partial re-ingest \
                                             ({} level(s) changed)",
                                            changed_luns.len()
                                        );
                                        // Apply the cache first, then re-fetch only the changed levels.
                                        apply_cached_columns(&mut catalog, &cached.columns);
                                        let sum =
                                            mqo_mcp_server::catalog_ingest::ingest_live_metadata(
                                                &mut catalog,
                                                ex,
                                                &xmla_catalog,
                                                &cube,
                                                &cfg,
                                            );
                                        eprintln!(
                                            "mqo-mcp-server: live catalog ingestion (partial, MDSCHEMA): \
                                             {} measures ({} semi-additive), {} levels seen / {} mapped, \
                                             {} domains captured, {} over-cap, {} errored, {}ms",
                                            sum.measures_seen, sum.semi_additive_found,
                                            sum.levels_seen, sum.levels_mapped,
                                            sum.domains_captured, sum.over_cap,
                                            sum.errored, sum.wall_ms
                                        );
                                        did_full_ingest = true;
                                    }
                                    CacheVerdict::FullReingest => {
                                        let sum =
                                            mqo_mcp_server::catalog_ingest::ingest_live_metadata(
                                                &mut catalog,
                                                ex,
                                                &xmla_catalog,
                                                &cube,
                                                &cfg,
                                            );
                                        eprintln!(
                                            "mqo-mcp-server: live catalog ingestion (full re-ingest, MDSCHEMA): \
                                             {} measures ({} semi-additive), {} levels seen / {} mapped, \
                                             {} domains captured, {} over-cap, {} errored, {}ms",
                                            sum.measures_seen, sum.semi_additive_found,
                                            sum.levels_seen, sum.levels_mapped,
                                            sum.domains_captured, sum.over_cap,
                                            sum.errored, sum.wall_ms
                                        );
                                        did_full_ingest = true;
                                    }
                                }
                            }
                        } else {
                            // No cache (first run, corrupt, or --refresh-catalog): full ingest.
                            let sum = mqo_mcp_server::catalog_ingest::ingest_live_metadata(
                                &mut catalog,
                                ex,
                                &xmla_catalog,
                                &cube,
                                &cfg,
                            );
                            eprintln!(
                                "mqo-mcp-server: live catalog ingestion (MDSCHEMA): \
                                 {} measures ({} semi-additive), {} levels seen / {} mapped, \
                                 {} domains captured, {} over-cap, {} errored, {}ms",
                                sum.measures_seen, sum.semi_additive_found, sum.levels_seen,
                                sum.levels_mapped, sum.domains_captured, sum.over_cap,
                                sum.errored, sum.wall_ms
                            );
                            did_full_ingest = true;
                        }

                        // ── Write cache after any ingest ──────────────────
                        // We only write after a full or partial ingest (not if
                        // we served a Valid cache unchanged — the file is still current).
                        if did_full_ingest || existing_cache.is_none() {
                            if let Some(cols) = catalog.get("columns").cloned() {
                                // Fetch fresh schema_update for the new cache.
                                let schema_update =
                                    fetch_schema_update(ex, &xmla_catalog, &cube);
                                let new_cache = CatalogCache {
                                    format_version: CACHE_FORMAT_VERSION,
                                    cube: cube.clone(),
                                    schema_update,
                                    captured_at: now_secs,
                                    columns: cols,
                                };
                                save_cache(&cache_path, &new_cache);
                                eprintln!(
                                    "mqo-mcp-server: catalog cache: written to {}",
                                    cache_path.display()
                                );
                            }
                        }
                    }
                    None => {
                        eprintln!(
                            "mqo-mcp-server: --capture-live-domains: no XMLA coords for model \
                             {model:?}; skipping ingestion"
                        );
                    }
                }
            }
            ServerEngine::Fixture => {
                eprintln!("mqo-mcp-server: --capture-live-domains ignored (fixture mode)");
            }
        }
    }

    // ── Enriched catalog (optional; graceful degradation on failure) ──────
    let enriched = build_enriched(&args, &catalog);

    // FR-1 (PRD-mqo-projection-handle-over-cap): The projection guard cap and the
    // materialization budget must be the same value — one knob (OQ-2).  In live mode,
    // clamp `max_result_rows` the same way `build_engine` does and use it as the
    // effective projection cap.  In fixture mode, the `--max-projection-cardinality`
    // arg defaults to `DEFAULT_MAX_PROJECTION_CARDINALITY` (= DEFAULT_MAX_RESULT_ROWS)
    // and the budget is `DEFAULT_MAX_RESULT_ROWS` — they agree by construction.
    let effective_max_projection = if args.endpoint.is_some() {
        args.max_result_rows
            .clamp(1, mqo_mcp_server::MAX_RESULT_ROWS_CEILING)
    } else {
        args.max_projection_cardinality
    };
    eprintln!("mqo-mcp-server: projection guard cap: {effective_max_projection}");

    // ── Auto-lift tier (OSL #2) ───────────────────────────────────────────────
    // Wire the base URL and an empty cache when auto-lift is enabled. The cache
    // is populated lazily on first OSL tool call per model.
    //
    // Resolve the XMLA URL the same way build_engine did so we can derive the
    // autolift base URL from it when --autolift-base-url is not set.
    let resolved_xmla_url_for_autolift = {
        let host = args
            .endpoint
            .as_deref()
            .and_then(|ep| parse_endpoint(ep).ok())
            .map(|(h, _)| h)
            .unwrap_or_default();
        resolve_xmla_url(args.xmla_url.as_deref(), &host)
    };
    let (autolift_base_url, autolift_cache) = build_autolift(&args, &resolved_xmla_url_for_autolift);
    if let Some(ref u) = autolift_base_url {
        eprintln!("mqo-mcp-server: autolift: enabled (base URL: {u})");
    } else {
        eprintln!("mqo-mcp-server: autolift: disabled (no --autolift-base-url and could not derive from --xmla-url)");
    }

    let server = Server {
        catalog,
        stats,
        tools,
        row_threshold: args.row_threshold,
        engine,
        backend_override: args.force_backend.clone(),
        capabilities,
        registry,
        health_cache,
        handle_store: Some(mqo_mcp_server::HandleStore::new()),
        cursor_store: Some(cursor_store),
        page_size: args.page_size,
        inline_threshold: args.inline_threshold,
        enriched,
        xmla_model_coords,
        max_projection_cardinality: effective_max_projection,
        // Static pre-loaded graph (fixture/test mode). In live mode with
        // auto-lift, graphs are loaded lazily via autolift_base_url.
        model_graph: None,
        // aso-ground overlay (OSL #3) not yet deployed; grounding_store is populated
        // when the grounding pipeline is integrated at server startup.
        grounding_store: None,
        // Ontology check store (OBQC advisory tier): populated when the lift
        // pipeline is integrated.  Fail-open until then (FR7).
        ontology_check: None,
        autolift_base_url,
        autolift_cache,
    };

    serve(&server);
}

/// Load JSON from `path`, exiting with code 2 on any error.
fn load_json(path: &std::path::Path) -> Value {
    read_json(path).unwrap_or_else(|e| {
        eprintln!("mqo-mcp-server: {e}");
        process::exit(2);
    })
}

/// Build the engine from CLI args: `Live` when `--endpoint` is set, `Fixture` otherwise.
fn build_engine(args: &Args) -> ServerEngine {
    let Some(ref endpoint_str) = args.endpoint else {
        eprintln!("mqo-mcp-server: engine: fixture (no endpoint configured)");
        return ServerEngine::Fixture;
    };

    let (pgwire_host, pgwire_port) = parse_endpoint(endpoint_str).unwrap_or_else(|e| {
        eprintln!("mqo-mcp-server: invalid --endpoint '{endpoint_str}': {e}");
        process::exit(2);
    });

    // Resolve direct-credential overrides, if any.
    let (pg_user, pg_pass) = match (&args.pg_user, &args.pg_pass_env) {
        (Some(user), Some(env_var)) => {
            let pass = std::env::var(env_var).unwrap_or_else(|_| {
                eprintln!(
                    "mqo-mcp-server: env var '{env_var}' (--pg-pass-env) is not set"
                );
                process::exit(2);
            });
            (Some(user.clone()), Some(pass))
        }
        (Some(_), None) => {
            eprintln!("mqo-mcp-server: --pg-user requires --pg-pass-env");
            process::exit(2);
        }
        (None, Some(_)) => {
            eprintln!("mqo-mcp-server: --pg-pass-env requires --pg-user");
            process::exit(2);
        }
        (None, None) => (None, None),
    };

    // `direct_auth` gates ONLY the PGWire executor's credential source. It must
    // NOT suppress OIDC token-provider construction: the XMLA path (DAX/MDX)
    // always authenticates with an OIDC bearer token, even when PGWire uses
    // direct credentials (PRD-mqo-mcp-server-xmla-oidc-auth, FR1/FR3).
    let direct_auth = pg_pass.is_some();

    // Build the OIDC config whenever the OIDC flags are present, regardless of
    // PGWire auth mode. When OIDC flags are absent AND direct PGWire creds are
    // in use, OIDC is unconfigured (SQL-only back-compat). When neither OIDC
    // flags nor direct creds are present, OIDC is required (pure-OIDC PGWire).
    let oidc = match build_oidc_config(args, direct_auth, |var| std::env::var(var).ok()) {
        Ok(cfg) => cfg,
        Err(msg) => {
            eprintln!("mqo-mcp-server: {msg}");
            process::exit(2);
        }
    };
    // Whether an OIDC token provider was actually configured (drives the
    // XMLA fail-fast probe and the "skipping OIDC" log line below).
    let oidc_configured = !oidc.token_url.is_empty();
    // ROPC is selected when an OIDC username is present (for the log line only).
    let oidc_ropc = oidc.username.is_some();
    // Non-secret username for the direct-auth log line; computed before `pg_user`
    // is moved into the EndpointConfig.
    let pg_user_log = pg_user.clone().unwrap_or_else(|| "token".to_string());

    // Resolve the XMLA URL for the primary DAX path. When the operator does not
    // pass --xmla-url, derive `https://<host>/v1/xmla` from the endpoint host so
    // DAX/MDX reach the engine without memorizing the path. An empty URL is
    // never silently kept: if a DAX/MDX query is later dispatched against an
    // empty URL the bridge surfaces a structured error.
    let xmla_url = resolve_xmla_url(args.xmla_url.as_deref(), &pgwire_host);
    if xmla_url.is_empty() {
        eprintln!(
            "mqo-mcp-server: xmla: no --xmla-url and could not derive from host \
             '{pgwire_host}'; DAX/MDX queries will fail with a structured error \
             naming /v1/xmla. Pass --xmla-url https://<host>/v1/xmla to enable them."
        );
    } else {
        eprintln!("mqo-mcp-server: xmla: {xmla_url}");
    }

    // Clamp the materialization budget to a sane, engine-deliverable range:
    // never 0 (would persist empty handles), never above the upstream PGWire
    // ceiling (NFR-1 — the bridge must not promise more than the engine can
    // deliver). A clamp (not a hard error) keeps startup robust; the effective
    // value is logged so the operator sees any adjustment.
    let max_result_rows = args
        .max_result_rows
        .clamp(1, mqo_mcp_server::MAX_RESULT_ROWS_CEILING);
    if max_result_rows != args.max_result_rows {
        eprintln!(
            "mqo-mcp-server: --max-result-rows {} out of range; clamped to {} \
             (1..={})",
            args.max_result_rows, max_result_rows, mqo_mcp_server::MAX_RESULT_ROWS_CEILING
        );
    }
    eprintln!("mqo-mcp-server: materialization budget: max_result_rows={max_result_rows}");

    let config = EndpointConfig {
        pgwire_host,
        pgwire_port,
        xmla_url,
        oidc,
        pg_user,
        pg_pass,
        max_result_rows,
    };

    let executor = LiveExecutor::new(config);

    if direct_auth {
        eprintln!("mqo-mcp-server: auth: PGWire direct credentials (user '{}')", pg_user_log);
    }

    if oidc_configured {
        // Fail-fast OIDC check: one token fetch before serving any request. This
        // runs even when PGWire uses direct creds, because the XMLA executor
        // (DAX/MDX) needs a working bearer token (FR1/FR4/FR5).
        let flow = if oidc_ropc { "ROPC (grant_type=password)" } else { "client_credentials" };
        match executor.fetch_token_sync() {
            Ok(token) => {
                let remaining = token
                    .expires_at
                    .saturating_duration_since(std::time::Instant::now());
                eprintln!(
                    "mqo-mcp-server: auth: XMLA OIDC ok via {flow} (token expires in {}s)",
                    remaining.as_secs()
                );
            }
            Err(e) => {
                eprintln!("mqo-mcp-server: XMLA OIDC auth error at startup: {e}");
                process::exit(1);
            }
        }
    } else if direct_auth {
        eprintln!(
            "mqo-mcp-server: auth: no OIDC flags set; XMLA (DAX/MDX) disabled, SQL only"
        );
    }

    ServerEngine::Live(Box::new(executor))
}

/// Build the OIDC config for the XMLA token provider from CLI args.
///
/// This is decoupled from PGWire auth (PRD-mqo-mcp-server-xmla-oidc-auth, FR1):
/// the OIDC provider is constructed whenever the OIDC flags are present, even
/// when `--pg-user`/`--pg-pass-env` direct credentials are in use for SQL.
///
/// Returns:
/// - An OIDC config with `client_credentials` (default) or ROPC
///   `grant_type=password` (when `--oidc-username` + `--oidc-password-env` are
///   set) when the OIDC flags are present.
/// - An *empty* OIDC config (all fields blank) when no OIDC flags are present
///   AND `direct_auth` is true — this is the SQL-only back-compat path; the
///   XMLA executor will have no token provider, exactly as before.
/// - `Err(message)` on misconfiguration: a partial OIDC flag set when no direct
///   creds back it; `--oidc-username` without `--oidc-password-env`; or
///   `--oidc-username` whose password env var is unset/empty (FR5 fail-fast).
///
/// Secret hygiene (NFR1): only the *names* of env vars are taken from the CLI;
/// password/secret *values* are read from the environment via `env_lookup`,
/// never from a flag. `env_lookup` returns `Some(value)` when the named var is
/// set (production passes `std::env::var(..).ok()`); tests pass a fake map so
/// the function stays pure and avoids mutating the process environment.
fn build_oidc_config(
    args: &Args,
    direct_auth: bool,
    env_lookup: impl Fn(&str) -> Option<String>,
) -> Result<OidcConfig, String> {
    let any_oidc = args.oidc_token_url.is_some()
        || args.oidc_client_id.is_some()
        || args.oidc_client_secret_env.is_some()
        || args.oidc_realm.is_some()
        || args.oidc_username.is_some()
        || args.oidc_password_env.is_some();

    // No OIDC flags at all: when direct PGWire creds carry SQL, OIDC is simply
    // unconfigured (today's SQL-only behavior). When no direct creds either, the
    // pure-OIDC PGWire path requires the OIDC flags — surface the missing one.
    if !any_oidc {
        if direct_auth {
            return Ok(OidcConfig {
                token_url: String::new(),
                client_id: String::new(),
                client_secret_env_var: String::new(),
                realm: String::new(),
                username: None,
                password_env_var: None,
            });
        }
        return Err(
            "--oidc-token-url is required when --endpoint is set without --pg-user/--pg-pass-env"
                .to_string(),
        );
    }

    // OIDC flags are present (in whole or in part): all four core fields are
    // required; report the first missing one clearly.
    let token_url = require_oidc_field(args.oidc_token_url.clone(), "--oidc-token-url")?;
    let client_id = require_oidc_field(args.oidc_client_id.clone(), "--oidc-client-id")?;
    let client_secret_env_var =
        require_oidc_field(args.oidc_client_secret_env.clone(), "--oidc-client-secret-env")?;
    let realm = require_oidc_field(args.oidc_realm.clone(), "--oidc-realm")?;

    // ROPC selection: --oidc-username requires --oidc-password-env, and the
    // named env var must be set and non-empty (FR2/FR5 fail-fast at startup).
    let (username, password_env_var) = match &args.oidc_username {
        Some(user) => {
            let pw_var = args.oidc_password_env.clone().ok_or_else(|| {
                "--oidc-username requires --oidc-password-env".to_string()
            })?;
            match env_lookup(&pw_var) {
                Some(v) if !v.is_empty() => {}
                _ => {
                    return Err(format!(
                        "--oidc-username is set but the password env var '{pw_var}' \
                         (--oidc-password-env) is unset or empty"
                    ));
                }
            }
            (Some(user.clone()), Some(pw_var))
        }
        None => {
            // --oidc-password-env without --oidc-username is meaningless; flag it.
            if args.oidc_password_env.is_some() {
                return Err(
                    "--oidc-password-env requires --oidc-username".to_string(),
                );
            }
            (None, None)
        }
    };

    Ok(OidcConfig {
        token_url,
        client_id,
        client_secret_env_var,
        realm,
        username,
        password_env_var,
    })
}

/// Return the OIDC flag value, or an `Err` message instead of exiting, so the
/// OIDC config builder stays pure and unit-testable.
fn require_oidc_field(val: Option<String>, flag: &str) -> Result<String, String> {
    val.ok_or_else(|| format!("{flag} is required when --endpoint is set"))
}

/// Build the auto-lift base URL and cache from CLI args.
///
/// Priority order:
/// 1. Explicit `--autolift-base-url` / `ATSCALE_CATALOG_XML_BASE` (takes
///    precedence over everything).
/// 2. Derived from `--xmla-url` (or the URL derived from `--endpoint` host):
///    strips a trailing `/v1/xmla` or `/xmla` suffix and replaces it with
///    `/v1/catalogs` so that `GET {base}/{catalogId}.xml` reaches the engine's
///    catalog REST endpoint without requiring an extra flag.
/// 3. Neither available → auto-lift disabled (`None, None`).
///
/// `resolved_xmla_url` is the XMLA URL already resolved by `build_engine` (may
/// be empty when derivation from the endpoint was also impossible).
fn build_autolift(
    args: &Args,
    resolved_xmla_url: &str,
) -> (Option<String>, Option<std::sync::Arc<AutoliftCache>>) {
    use mqo_mcp_server::autolift::derive_autolift_base_url;

    // 1. Explicit flag wins unconditionally.
    if let Some(url) = &args.autolift_base_url {
        let trimmed = url.trim().to_string();
        if !trimmed.is_empty() {
            return (Some(trimmed), Some(std::sync::Arc::new(AutoliftCache::new())));
        }
    }

    // 2. Derive from the resolved XMLA URL.
    if let Some(derived) = derive_autolift_base_url(resolved_xmla_url) {
        return (Some(derived), Some(std::sync::Arc::new(AutoliftCache::new())));
    }

    // 3. Disabled.
    (None, None)
}

/// Drive the server loop: read JSON-RPC from stdin, write responses to stdout.
fn serve(server: &Server) {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let req: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                let err = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32700, "message": format!("parse error: {e}") }
                });
                let _ = writeln!(out, "{err}");
                let _ = out.flush();
                continue;
            }
        };

        if let Some(resp) = server.handle(&req) {
            if writeln!(out, "{resp}").is_err() {
                break;
            }
            let _ = out.flush();
        }
    }
}

/// Resolve the HTTP XMLA engine URL for the primary DAX path.
///
/// - An explicit, non-empty `--xmla-url` always wins (after trimming).
/// - Otherwise the URL is derived from the `PGWire` endpoint host as
///   `https://<host>/v1/xmla` — the engine's HTTP XMLA path. (`/engine/xmla/`,
///   `/xmla`, and `/dax` route to the Modeler app, not the engine.)
/// - If no host is available the result is empty; the caller warns and the
///   bridge surfaces a structured error rather than `POST`ing to an empty URL.
fn resolve_xmla_url(explicit: Option<&str>, pgwire_host: &str) -> String {
    if let Some(url) = explicit {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let host = pgwire_host.trim();
    if host.is_empty() {
        return String::new();
    }
    format!("https://{host}/v1/xmla")
}

/// Parse `host:port` into a `(String, u16)` pair.
fn parse_endpoint(s: &str) -> Result<(String, u16), String> {
    let (host, port_str) = s
        .rsplit_once(':')
        .ok_or_else(|| "expected format <host:port>, e.g. localhost:15432".to_string())?;
    let port: u16 = port_str
        .parse()
        .map_err(|e| format!("invalid port '{port_str}': {e}"))?;
    Ok((host.to_string(), port))
}

fn read_json(path: &std::path::Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", path.display()))
}

/// Select enrichment source: explicit file, auto-derive, or None (warn + degrade).
fn build_enriched(args: &Args, catalog: &Value) -> Option<Arc<ServerEnrichedData>> {
    if let Some(ref path) = args.enriched_catalog {
        try_load_enriched_from_file(path)
    } else if let Some(data) = try_auto_derive_enriched(catalog) {
        Some(data)
    } else {
        eprintln!(
            "mqo-mcp-server: WARN: catalog enrichment unavailable; \
             CrossFactPath checking disabled (raw-catalog mode)"
        );
        None
    }
}

fn try_load_enriched_from_file(path: &std::path::Path) -> Option<Arc<ServerEnrichedData>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!(
                "mqo-mcp-server: WARN: cannot read --enriched-catalog '{}': {e}",
                path.display()
            );
            return None;
        }
    };
    let json: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "mqo-mcp-server: WARN: --enriched-catalog '{}' is not valid \
                 enriched-catalog.v1 JSON: {e}",
                path.display()
            );
            return None;
        }
    };
    if let Some(data) = ServerEnrichedData::from_json(&json) {
        eprintln!("mqo-mcp-server: catalog mode: enriched (loaded from --enriched-catalog)");
        Some(Arc::new(data))
    } else {
        eprintln!(
            "mqo-mcp-server: WARN: --enriched-catalog '{}' produced no enrichment data",
            path.display()
        );
        None
    }
}

fn try_auto_derive_enriched(catalog: &Value) -> Option<Arc<ServerEnrichedData>> {
    use mqoguard_column_group_enrichment::{enrich, CatalogSnapshot, FactBindings};
    let snap: CatalogSnapshot = serde_json::from_value(catalog.clone()).ok()?;
    let bindings = FactBindings::tpcds_defaults();
    let enriched = enrich(&snap, &bindings);
    let enriched_json = serde_json::to_value(&enriched).ok()?;
    let data = ServerEnrichedData::from_json(&enriched_json)?;
    eprintln!("mqo-mcp-server: catalog mode: enriched (derived)");
    Some(Arc::new(data))
}

/// Build the XMLA model coordinate map.
///
/// Priority order:
/// 1. Static `--xmla-catalog-map` JSON file (explicit overrides discovery).
/// 2. Auto-discovery via `DBSCHEMA_CATALOGS` + `MDSCHEMA_CUBES` when in live
///    mode and `--xmla-url` (or derived URL) is available.
/// 3. Empty map (fixture mode, or live mode with no discovery configured).
fn build_xmla_model_coords(
    args: &Args,
    engine: &ServerEngine,
) -> HashMap<String, (String, String)> {
    // 1. Static map from file.
    if let Some(ref map_path) = args.xmla_catalog_map {
        match load_static_catalog_map(map_path) {
            Some(m) => {
                eprintln!(
                    "mqo-mcp-server: xmla-catalog-map: loaded {} entry(s) from {}",
                    m.len(),
                    map_path.display()
                );
                return m;
            }
            None => {
                eprintln!(
                    "mqo-mcp-server: WARN: --xmla-catalog-map '{}' failed to load; \
                     falling through to auto-discovery",
                    map_path.display()
                );
            }
        }
    }

    // 2. Auto-discovery (live mode only, requires bearer token + xmla_url).
    if let ServerEngine::Live(ex) = engine {
        let xmla_url = match &args.endpoint {
            Some(ep) => {
                // Re-derive the XMLA URL the same way build_engine did.
                let (host, _) = parse_endpoint(ep)
                    .unwrap_or_else(|_| (String::new(), 0));
                resolve_xmla_url(args.xmla_url.as_deref(), &host)
            }
            None => String::new(),
        };

        if xmla_url.is_empty() {
            eprintln!(
                "mqo-mcp-server: xmla-catalog-map: no URL available for discovery; \
                 DAX/MDX will fail with xmla_coords_not_found"
            );
            return HashMap::new();
        }

        match ex.fetch_token_sync() {
            Ok(token) => {
                eprintln!("mqo-mcp-server: xmla-catalog-map: running discovery against {xmla_url}");
                return discover_xmla_coords(&xmla_url, &token.access_token);
            }
            Err(e) => {
                eprintln!(
                    "mqo-mcp-server: WARN: cannot fetch token for XMLA discovery: {e}; \
                     DAX/MDX will fail with xmla_coords_not_found"
                );
            }
        }
    }

    // 3. Fixture mode or discovery unavailable — empty map.
    HashMap::new()
}

/// Load a static XMLA catalog map from a JSON file.
///
/// Accepts the format:
/// ```json
/// {"cube_name": {"catalog": "xmla_catalog", "cube": "cube_name"}, …}
/// ```
///
/// Returns `None` on read or parse failure (caller logs and falls through).
fn load_static_catalog_map(path: &std::path::Path) -> Option<HashMap<String, (String, String)>> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        eprintln!(
            "mqo-mcp-server: WARN: cannot read --xmla-catalog-map '{}': {e}",
            path.display()
        );
    }).ok()?;
    let json: Value = serde_json::from_str(&text).map_err(|e| {
        eprintln!(
            "mqo-mcp-server: WARN: --xmla-catalog-map '{}' is not valid JSON: {e}",
            path.display()
        );
    }).ok()?;
    let obj = json.as_object()?;
    let mut map = HashMap::new();
    for (cube_name, entry) in obj {
        let catalog = entry.get("catalog").and_then(Value::as_str)?;
        let cube = entry.get("cube").and_then(Value::as_str)?;
        map.insert(cube_name.clone(), (catalog.to_string(), cube.to_string()));
    }
    Some(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_xmla_url_wins() {
        let got = resolve_xmla_url(
            Some("https://example.com/v1/xmla"),
            "mcp-aws.atscaleinternal.com",
        );
        assert_eq!(got, "https://example.com/v1/xmla");
    }

    #[test]
    fn derives_v1_xmla_from_endpoint_host() {
        // No --xmla-url: derive https://<host>/v1/xmla so DAX reaches the engine.
        let got = resolve_xmla_url(None, "mcp-aws.atscaleinternal.com");
        assert_eq!(got, "https://mcp-aws.atscaleinternal.com/v1/xmla");
    }

    #[test]
    fn empty_explicit_falls_back_to_derivation() {
        let got = resolve_xmla_url(Some("   "), "mcp-aws.atscaleinternal.com");
        assert_eq!(got, "https://mcp-aws.atscaleinternal.com/v1/xmla");
    }

    #[test]
    fn no_host_yields_empty() {
        // Caller warns and the bridge surfaces a structured error on DAX/MDX.
        assert!(resolve_xmla_url(None, "").is_empty());
        assert!(resolve_xmla_url(Some(""), "  ").is_empty());
    }

    #[test]
    fn explicit_url_is_trimmed() {
        let got = resolve_xmla_url(Some("  https://h/v1/xmla  "), "ignored");
        assert_eq!(got, "https://h/v1/xmla");
    }

    /// FR2 wiring: the derived URL must be the engine path `/v1/xmla`, never one
    /// of the Modeler-app paths.
    #[test]
    fn derived_url_uses_v1_xmla_not_modeler_paths() {
        let got = resolve_xmla_url(None, "host");
        assert!(got.ends_with("/v1/xmla"), "uses engine path: {got}");
        assert!(!got.contains("/engine/xmla"), "not the Modeler app path");
    }

    // ── PRD-mqo-mcp-server-xmla-oidc-auth: OIDC config decoupling ──────────

    use clap::Parser;

    /// Parse `Args` from a synthetic CLI line. The binary name and the required
    /// `--catalog` arg are prepended so only the auth flags vary per test.
    fn args_from(extra: &[&str]) -> Args {
        let mut v = vec!["mqo-mcp-server", "--catalog", "/dev/null"];
        v.extend_from_slice(extra);
        Args::parse_from(v)
    }

    const OIDC_FLAGS: &[&str] = &[
        "--oidc-token-url",
        "https://idp/token",
        "--oidc-client-id",
        "atscale-modeler",
        "--oidc-client-secret-env",
        "ATSCALE_OIDC_SECRET",
        "--oidc-realm",
        "atscale",
    ];

    /// Env lookup that always misses — every var unset (no process mutation).
    fn env_empty(_: &str) -> Option<String> {
        None
    }

    /// Env lookup that returns a non-empty value for `var`, else `None`.
    fn env_with(var: &'static str, val: &'static str) -> impl Fn(&str) -> Option<String> {
        move |q: &str| (q == var).then(|| val.to_string())
    }

    /// AC#1: PGWire direct creds AND OIDC flags → OIDC provider IS constructed
    /// for XMLA (token_url present), decoupled from direct_auth.
    #[test]
    fn oidc_built_even_with_direct_pgwire_auth() {
        let mut argv = vec!["--pg-user", "joe", "--pg-pass-env", "PG_PASS"];
        argv.extend_from_slice(OIDC_FLAGS);
        let args = args_from(&argv);
        // direct_auth = true (pg creds present), yet OIDC must still build.
        let cfg = build_oidc_config(&args, true, env_empty).expect("oidc config builds");
        assert_eq!(cfg.token_url, "https://idp/token");
        assert_eq!(cfg.client_id, "atscale-modeler");
        assert_eq!(cfg.client_secret_env_var, "ATSCALE_OIDC_SECRET");
        // client_credentials flow (no ROPC username).
        assert!(cfg.username.is_none());
    }

    /// AC#2: --oidc-username + --oidc-password-env → ROPC flow selected.
    #[test]
    fn oidc_username_selects_ropc() {
        let mut argv = vec!["--oidc-username", "modeler-user", "--oidc-password-env", "ROPC_PW"];
        argv.extend_from_slice(OIDC_FLAGS);
        let args = args_from(&argv);
        let cfg = build_oidc_config(&args, false, env_with("ROPC_PW", "hunter2"))
            .expect("ropc config builds");
        assert_eq!(cfg.username.as_deref(), Some("modeler-user"));
        assert_eq!(cfg.password_env_var.as_deref(), Some("ROPC_PW"));
    }

    /// AC#3 / AC#7 back-compat: only direct PGWire creds, no OIDC flags →
    /// OIDC unconfigured (empty), identical to today's SQL-only behavior.
    #[test]
    fn sql_only_leaves_oidc_unconfigured() {
        let args = args_from(&["--pg-user", "joe", "--pg-pass-env", "PG_PASS"]);
        let cfg = build_oidc_config(&args, true, env_empty).expect("sql-only builds");
        assert!(cfg.token_url.is_empty(), "no OIDC provider for SQL-only");
        assert!(cfg.username.is_none());
    }

    /// AC#7 back-compat: pure-OIDC (no direct creds) still builds the provider.
    #[test]
    fn pure_oidc_builds_client_credentials() {
        let args = args_from(OIDC_FLAGS);
        let cfg = build_oidc_config(&args, false, env_empty).expect("pure-oidc builds");
        assert_eq!(cfg.token_url, "https://idp/token");
        assert!(cfg.username.is_none(), "client_credentials, not ROPC");
    }

    /// AC#4: --oidc-username set but password env var absent → fail fast with a
    /// message naming the missing env var.
    #[test]
    fn ropc_missing_password_env_fails_fast() {
        let mut argv = vec!["--oidc-username", "u", "--oidc-password-env", "ROPC_PW_ABSENT"];
        argv.extend_from_slice(OIDC_FLAGS);
        let args = args_from(&argv);
        let err = build_oidc_config(&args, false, env_empty).expect_err("must fail fast");
        assert!(err.contains("ROPC_PW_ABSENT"), "names the env var: {err}");
    }

    /// AC#4 edge: --oidc-username set but password env var present-but-EMPTY →
    /// still fails fast (empty is treated as unset).
    #[test]
    fn ropc_empty_password_env_fails_fast() {
        let mut argv = vec!["--oidc-username", "u", "--oidc-password-env", "ROPC_PW_EMPTY"];
        argv.extend_from_slice(OIDC_FLAGS);
        let args = args_from(&argv);
        let err = build_oidc_config(&args, false, env_with("ROPC_PW_EMPTY", ""))
            .expect_err("empty password fails");
        assert!(err.contains("ROPC_PW_EMPTY"), "names the env var: {err}");
    }

    /// AC#4 variant: --oidc-username without --oidc-password-env → clear error.
    #[test]
    fn ropc_username_without_password_env_flag_fails() {
        let mut argv = vec!["--oidc-username", "u"];
        argv.extend_from_slice(OIDC_FLAGS);
        let args = args_from(&argv);
        let err = build_oidc_config(&args, false, env_empty).expect_err("must fail");
        assert!(err.contains("--oidc-password-env"), "names the flag: {err}");
    }

    /// Partial OIDC flag set (no direct creds to fall back to) → names the
    /// first missing required field rather than silently degrading.
    #[test]
    fn partial_oidc_flags_report_missing_field() {
        let args = args_from(&["--oidc-token-url", "https://idp/token"]);
        let err = build_oidc_config(&args, false, env_empty).expect_err("incomplete oidc");
        assert!(err.contains("--oidc-client-id"), "names missing field: {err}");
    }

    /// AC#5 secret hygiene: the password/secret flags take VAR NAMES, not
    /// values. Verify the config stores the env-var name, never a secret value.
    #[test]
    fn flags_store_env_var_names_not_secrets() {
        let mut argv = vec!["--oidc-username", "u", "--oidc-password-env", "ROPC_PW"];
        argv.extend_from_slice(OIDC_FLAGS);
        let args = args_from(&argv);
        let cfg = build_oidc_config(&args, false, env_with("ROPC_PW", "topsecret-value"))
            .expect("builds");
        assert_eq!(cfg.password_env_var.as_deref(), Some("ROPC_PW"));
        assert_eq!(cfg.client_secret_env_var, "ATSCALE_OIDC_SECRET");
        // The secret VALUE must never be stored in the config struct.
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("topsecret-value"), "secret value leaked: {dbg}");
    }

    // ── build_autolift: URL derivation ────────────────────────────────────────

    /// Explicit --autolift-base-url always wins over xmla-url derivation.
    #[test]
    fn build_autolift_explicit_flag_wins_over_derivation() {
        let args = args_from(&["--autolift-base-url", "https://explicit.example.com/api/1.0"]);
        let xmla_url = "https://mcp-aws.atscaleinternal.com/v1/xmla";
        let (base, cache) = build_autolift(&args, xmla_url);
        assert_eq!(
            base.as_deref(),
            Some("https://explicit.example.com/api/1.0"),
            "explicit flag must win over xmla_url derivation"
        );
        assert!(cache.is_some(), "cache must be Some when base URL is set");
    }

    /// When --autolift-base-url is absent, derive from --xmla-url.
    #[test]
    fn build_autolift_derives_from_xmla_url_when_no_explicit_flag() {
        let args = args_from(&[]);
        let xmla_url = "https://mcp-aws.atscaleinternal.com/v1/xmla";
        let (base, cache) = build_autolift(&args, xmla_url);
        assert_eq!(
            base.as_deref(),
            Some("https://mcp-aws.atscaleinternal.com/v1/catalogs"),
            "must derive /v1/catalogs from /v1/xmla"
        );
        assert!(cache.is_some(), "cache must be Some when derivation succeeds");
    }

    /// Neither explicit flag nor derivable xmla_url → autolift disabled.
    #[test]
    fn build_autolift_disabled_when_neither_flag_nor_xmla_url() {
        let args = args_from(&[]);
        let (base, cache) = build_autolift(&args, "");
        assert!(base.is_none(), "must be None when no URL available");
        assert!(cache.is_none(), "cache must be None when disabled");
    }

    /// Empty explicit flag falls through to xmla_url derivation (not to disabled).
    #[test]
    fn build_autolift_empty_explicit_falls_through_to_derivation() {
        // An empty --autolift-base-url is treated as "not set" (same as None).
        let args = args_from(&["--autolift-base-url", "   "]);
        let xmla_url = "https://mcp-aws.atscaleinternal.com/v1/xmla";
        let (base, _cache) = build_autolift(&args, xmla_url);
        assert_eq!(
            base.as_deref(),
            Some("https://mcp-aws.atscaleinternal.com/v1/catalogs"),
            "whitespace-only explicit flag must fall through to derivation"
        );
    }
}
