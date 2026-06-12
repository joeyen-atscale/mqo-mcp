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
#![allow(clippy::module_name_repetitions)]

use clap::Parser;
use mcp_cluster_health_monitor::report::OverallStatus;
use mcp_cluster_registry::ClusterRegistry;
use mqo_mcp_server::{
    cursor::{CursorStore, DEFAULT_CURSOR_TTL_SECS, DEFAULT_PAGE_SIZE},
    mcp::discover_xmla_coords,
    run_health_check_sync, BackendCapabilities, EndpointConfig, LiveExecutor, OidcConfig, Server,
    ServerEnrichedData, ServerEngine, ToolPaths,
};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process;
use std::sync::{Arc, Mutex};

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

    /// `PGWire` username override. When set, disables OIDC bearer-token auth and
    /// uses direct credentials instead. Must be paired with --pg-pass-env.
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
}

fn main() {
    let args = Args::parse();

    let catalog = load_json(&args.catalog);
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

    // ── Enriched catalog (optional; graceful degradation on failure) ──────
    let enriched = build_enriched(&args, &catalog);

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

    let direct_auth = pg_pass.is_some();

    // OIDC config is only required when direct-credential auth is not in use.
    let oidc = if direct_auth {
        OidcConfig {
            token_url: String::new(),
            client_id: String::new(),
            client_secret_env_var: String::new(),
            realm: String::new(),
            username: None,
            password_env_var: None,
        }
    } else {
        OidcConfig {
            token_url: require_flag(args.oidc_token_url.clone(), "--oidc-token-url"),
            client_id: require_flag(args.oidc_client_id.clone(), "--oidc-client-id"),
            client_secret_env_var: require_flag(
                args.oidc_client_secret_env.clone(),
                "--oidc-client-secret-env",
            ),
            realm: require_flag(args.oidc_realm.clone(), "--oidc-realm"),
            username: None,
            password_env_var: None,
        }
    };

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

    let config = EndpointConfig {
        pgwire_host,
        pgwire_port,
        xmla_url,
        oidc,
        pg_user,
        pg_pass,
    };

    let executor = LiveExecutor::new(config);

    if direct_auth {
        eprintln!("mqo-mcp-server: auth: direct credentials (skipping OIDC)");
    } else {
        // Fail-fast OIDC check: one token fetch before serving any request.
        match executor.fetch_token_sync() {
            Ok(token) => {
                let remaining = token
                    .expires_at
                    .saturating_duration_since(std::time::Instant::now());
                eprintln!(
                    "mqo-mcp-server: auth: ok (token expires in {}s)",
                    remaining.as_secs()
                );
            }
            Err(e) => {
                eprintln!("mqo-mcp-server: auth error at startup: {e}");
                process::exit(1);
            }
        }
    }

    ServerEngine::Live(Box::new(executor))
}

/// Return the value or exit with a helpful error if it's `None`.
fn require_flag(val: Option<String>, flag: &str) -> String {
    val.unwrap_or_else(|| {
        eprintln!("mqo-mcp-server: {flag} is required when --endpoint is set");
        process::exit(2);
    })
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
}
