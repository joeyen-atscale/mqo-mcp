//! The bind → route → compile → execute pipeline.
//!
//! The MQO fleet is a JSON pipeline of CLI subprocesses, not a library graph.
//! This module orchestrates the published fleet binaries:
//!
//! - `mqo-bind`  — `--mqo <f> --catalog <f>` → `BoundMqo` JSON (exit 0),
//!   `{"ambiguous":[...]}` (exit 3), `{"not_found":[...]}` (exit 4).
//! - `mqo-route` — `--bound <f> --stats <f>` → routing-decision JSON (exit 0).
//! - `mqo-dax`   — `--bound <f>` → DAX text on stdout (exit 0).
//! - `mqo-mdx`   — `--bound <f>` → MDX text on stdout (exit 0).
//!
//! JSON is exchanged on disk (temp files) because that is each tool's documented
//! CLI contract (`--mqo`, `--catalog`, `--bound`, `--stats` all take paths).

// Pre-existing lint suppressions — do not remove without fixing the underlying code.
#![allow(
    clippy::doc_markdown, clippy::missing_errors_doc, clippy::missing_panics_doc,
    clippy::must_use_candidate, clippy::map_unwrap_or, clippy::manual_let_else,
    clippy::items_after_statements, clippy::too_many_lines, clippy::uninlined_format_args,
    clippy::cast_possible_truncation, clippy::cast_precision_loss, clippy::implicit_hasher,
    clippy::similar_names, clippy::redundant_closure_for_method_calls, clippy::map_clone,
    clippy::if_not_else, clippy::unnested_or_patterns, clippy::manual_range_patterns,
    clippy::explicit_auto_deref, clippy::doc_overindented_list_items,
    clippy::used_underscore_binding, clippy::absurd_extreme_comparisons, clippy::type_complexity
)]

use mqo_auth_bridge::{Backend, EngineResult, FixtureEngine};
use mqoguard_filter_bind_report::{
    BoundMqo, CompiledQuery, MemberFilter, MqoFilter, report_filters,
};
use crate::probe::BackendCapabilities;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where to find the fleet binaries.
#[derive(Debug, Clone)]
pub struct ToolPaths {
    pub bind: PathBuf,
    pub route: PathBuf,
    pub dax: PathBuf,
    pub mdx: PathBuf,
}

impl ToolPaths {
    /// Resolve each tool by name, preferring an explicit release directory,
    /// then `$HOME/.local/bin`, then the bare name (PATH lookup at exec time).
    #[must_use]
    pub fn resolve(release_hint: Option<&Path>) -> Self {
        ToolPaths {
            bind: resolve_one("mqo-bind", release_hint),
            route: resolve_one("mqo-route", release_hint),
            dax: resolve_one("mqo-dax", release_hint),
            mdx: resolve_one("mqo-mdx", release_hint),
        }
    }
}

fn resolve_one(name: &str, release_hint: Option<&Path>) -> PathBuf {
    if let Some(dir) = release_hint {
        let p = dir.join(name);
        if p.exists() {
            return p;
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join(".local/bin").join(name);
        if p.exists() {
            return p;
        }
    }
    PathBuf::from(name)
}

/// Errors raised while running the pipeline.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// The submitted query was not a structurally valid MQO (e.g. a raw SQL
    /// string, or a JSON object that does not deserialize to [`mqo_spec::Mqo`]).
    #[error("not a valid MQO: {0}")]
    NotAnMqo(String),

    /// The MQO failed structural validation in `mqo-spec`.
    #[error("MQO structural validation failed: {0}")]
    Invalid(String),

    /// The binder could not ground one or more references.
    /// Carries the binder's structured report verbatim.
    #[error("binding failed")]
    NotGround { report: Value },

    /// A subprocess could not be launched or returned an unexpected status.
    #[error("subprocess `{tool}` failed: {detail}")]
    Subprocess { tool: String, detail: String },

    /// Temp-file or other I/O error.
    #[error("io error: {0}")]
    Io(String),

    /// Engine execution failed (auth, connection, or query error from the bridge).
    #[error("engine error: {0}")]
    Engine(#[from] mqo_auth_bridge::EngineError),

    /// The capability probe determined all backends are unavailable.
    #[error("no backend available: all backends are dead (dax={dax}, mdx={mdx}, sql={sql})")]
    NoBackendAvailable {
        /// DAX port status string.
        dax: String,
        /// MDX port status string.
        mdx: String,
        /// SQL port status string.
        sql: String,
    },

    /// The binder returned `Incompatible` (exit 5): one or more measure×dimension pairs span
    /// different fact tables. Carries the binder's structured `{"incompatible":[...]}` report.
    #[error("cross-fact incompatible path")]
    CrossFactIncompatible { report: Value },

    /// The pre-execution param-validator rejected one or more MQO fields
    /// (unmapped measure/dimension/filter, wrong hierarchy level, or a
    /// hand-rederived packaged calc). Carries the structured
    /// `{"param_rejections":[...]}` report so the model can retry with the
    /// suggested entity. No execution happens when this fires.
    #[error("param validation rejected {count} field(s) before execution")]
    ParamRejected {
        /// Number of rejected fields.
        count: usize,
        /// The structured rejection report (verbatim validator output).
        report: Value,
    },

    /// The XMLA coordinate map does not contain an entry for the requested model.
    /// The bare model name or `probe_model` placeholder cannot reach the executor.
    /// Populate `--xmla-catalog-map` or ensure XMLA discovery ran at startup.
    #[error("XMLA coordinates not found for model '{model}'")]
    XmlaCoordsNotFound {
        /// The model name that had no entry in the coordinate map.
        model: String,
    },

    /// The pre-execution projection cardinality guard declined the query because
    /// the estimated distinct-row count exceeds the configured cap.  No execution
    /// occurred (FR-3: no credits spent).
    #[error("projection_too_large: level '{level}' estimate {estimate} > cap {cap}")]
    ProjectionTooLarge {
        /// The first dimension level whose contribution pushed the estimate over cap.
        level: String,
        /// The total estimated distinct-row count.
        estimate: u64,
        /// The configured cap.
        cap: usize,
    },

    /// A projection MQO carries a `limit` but no `order`, making the top-N
    /// non-deterministic.  The caller must add a well-defined `order` so that
    /// the TOPN is deterministic.  No execution occurred.
    #[error("projection_unordered_limit: limit is set but order is absent — result is non-deterministic")]
    ProjectionUnorderedLimit,

    /// The requested model is a non-queryable dimension — it exists in the
    /// catalog but has no XMLA cube mapping.  The caller should retry against
    /// one of the cubes listed in `candidate_cubes`.
    ///
    /// Classified as `model_path` (not `infrastructure`) so the LLM can act on
    /// it in one retry rather than treating it as an opaque infra failure.
    #[error("model '{model}' is a dimension, not a queryable cube; use one of: {candidate_cubes:?}")]
    NonQueryableDimension {
        /// The dimension model name the caller requested.
        model: String,
        /// The queryable cube(s) that contain this dimension.
        candidate_cubes: Vec<String>,
    },

    /// The engine returned rows where one or more requested dimension columns
    /// are absent — a near-twin level (shared across hierarchies) or a DAX
    /// result that silently dropped a grouping column. Carrying a report with
    /// the requested dimensions and the columns that were actually returned.
    ///
    /// Classified as `model_path` so the LLM can act on it rather than treating
    /// it as an opaque infrastructure failure.
    #[error("dimension_not_materialized: engine result is missing {missing} of {requested} requested dimension column(s)")]
    DimensionNotMaterialized {
        /// Number of dimension columns that were dropped.
        missing: usize,
        /// Total number of requested dimensions.
        requested: usize,
        /// The structured report payload.
        report: Value,
    },

    /// The compiled SQL string failed SQL-level validation (e.g. multi-statement
    /// injection, ATSCALE-48466). Carries the structured rejection list so the
    /// agent can correct and retry. No execution occurs when this fires.
    #[error("sql_rejected: {count} SQL violation(s) detected before execution")]
    SqlRejected {
        /// Number of SQL violations.
        count: usize,
        /// Structured list of [`mqo_param_validator::sql_validator::SqlRejection`]
        /// items, each with a `rule` code and `message`.
        report: Value,
    },
}

// ── Error classification ──────────────────────────────────────────────────────

/// Stable machine-readable error class values emitted in MCP error responses.
pub mod error_class_values {
    /// The query failed because infrastructure was unavailable or misconfigured
    /// (backend down, XMLA coordinates missing, auth/transport failure).
    /// The model/path is not at fault.
    pub const INFRASTRUCTURE: &str = "infrastructure";

    /// The query failed because of a model or path problem (not-ground, invalid
    /// MQO structure, cross-fact incompatible path, subprocess failure).
    pub const MODEL_PATH: &str = "model_path";
}

/// Return a stable machine-readable class for a [`PipelineError`].
///
/// Classification is based on the enum variant — never on the rendered error
/// message string. This guarantees that rewording an `#[error(...)]` format
/// string cannot silently change the class emitted to callers.
///
/// # Class semantics
///
/// | Class | Meaning |
/// |-------|---------|
/// | `"infrastructure"` | Backend unavailable, XMLA coords missing, transport/auth failure |
/// | `"model_path"` | Query construction or binding failure — model or path is at fault |
#[must_use]
pub fn error_class(e: &PipelineError) -> &'static str {
    use error_class_values::{INFRASTRUCTURE, MODEL_PATH};
    match e {
        // Infrastructure: backend unavailable, coords missing, I/O, or engine
        // transport/auth failures (see engine_error_class for inner dispatch).
        PipelineError::NoBackendAvailable { .. }
        | PipelineError::XmlaCoordsNotFound { .. }
        | PipelineError::Io(_) => INFRASTRUCTURE,
        PipelineError::Engine(inner) => engine_error_class(inner),
        // Model/path: query construction, binding, or pre-execution guard failures.
        PipelineError::NotGround { .. }
        | PipelineError::CrossFactIncompatible { .. }
        | PipelineError::ParamRejected { .. }
        | PipelineError::ProjectionTooLarge { .. }
        | PipelineError::ProjectionUnorderedLimit
        | PipelineError::NonQueryableDimension { .. }
        | PipelineError::DimensionNotMaterialized { .. }
        | PipelineError::SqlRejected { .. }
        | PipelineError::Invalid(_)
        | PipelineError::NotAnMqo(_)
        | PipelineError::Subprocess { .. } => MODEL_PATH,
    }
}

/// Classify an [`mqo_auth_bridge::EngineError`] variant.
///
/// Transport, auth, and connection failures are infrastructure; query execution
/// errors are model/path (the query reached the engine but was rejected).
/// `RowCapTripped` is treated as infrastructure (a hard limit, not a query fault).
fn engine_error_class(e: &mqo_auth_bridge::EngineError) -> &'static str {
    use mqo_auth_bridge::EngineError;
    use error_class_values::{INFRASTRUCTURE, MODEL_PATH};
    match e {
        EngineError::QueryError { .. } => MODEL_PATH,
        // All transport, auth, connection, and resource-cap failures are
        // infrastructure; merging arms to satisfy clippy::match_same_arms.
        EngineError::MissingSecret { .. }
        | EngineError::AuthFailure { .. }
        | EngineError::ConnectionFailure { .. }
        | EngineError::Http(_)
        | EngineError::Postgres(_)
        | EngineError::RowCapTripped { .. }
        | EngineError::QueryDeadlineExceeded { .. }
        // Retried-and-exhausted is still infrastructure: the query shape was
        // correct (it was retried for transient errors) but the backend is flaky.
        | EngineError::EngineErrorRetriedExhausted { .. } => INFRASTRUCTURE,
    }
}

/// Successful pipeline output.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PipelineOutput {
    /// The backend chosen by the router (`dax` | `mdx` | `sql`).
    pub backend: String,
    /// Estimated row count from the router.
    pub estimated_rows: u64,
    /// Human-readable routing reason.
    pub routing_reason: String,
    /// The compiled query text (DAX, MDX, or the SQL projection).
    pub compiled_query: String,
    /// The bounded result rows from the fixture engine.
    pub rows: Vec<Value>,
    /// The fully-bound MQO (binder output), echoed for transparency.
    pub bound: Value,
    /// Filters from the MQO that were present in the compiled query.
    /// Always present (empty when the MQO had no filters or all were dropped).
    pub filters_applied: Vec<Value>,
    /// Filters from the MQO that were absent from the compiled query.
    /// Always present (empty when the MQO had no filters or all applied).
    pub filters_dropped: Vec<Value>,
    /// `true` when the engine's real result EXCEEDED the materialization budget
    /// and `rows` was therefore truncated to it. The response layer turns this
    /// into a typed `result_too_large` over-budget signal — the persisted handle
    /// and inline rows are an incomplete prefix, never the full answer
    /// (PRD-mqo-handle-full-materialization, FR-3).
    pub row_cap_tripped: bool,
}

/// A scratch directory whose temp files are cleaned up on drop.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new() -> Result<Self, PipelineError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        // A process-wide counter guarantees uniqueness even when parallel tests
        // call Scratch::new() within the same nanosecond on the same PID.
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir();
        let pid = std::process::id();
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = base.join(format!("mqo-mcp-{pid}-{nonce}-{seq}"));
        std::fs::create_dir_all(&dir).map_err(|e| PipelineError::Io(e.to_string()))?;
        Ok(Scratch { dir })
    }

    fn write(&self, name: &str, contents: &str) -> Result<PathBuf, PipelineError> {
        let p = self.dir.join(name);
        let mut f = std::fs::File::create(&p).map_err(|e| PipelineError::Io(e.to_string()))?;
        f.write_all(contents.as_bytes())
            .map_err(|e| PipelineError::Io(e.to_string()))?;
        Ok(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Run the full pipeline for a raw query value plus a catalog snapshot and stats.
///
/// `query` is the untrusted input from the MCP tool call. It MUST be a JSON
/// object that deserializes to an [`mqo_spec::Mqo`]; a raw SQL string (or any
/// other shape) is rejected with [`PipelineError::NotAnMqo`] before any
/// execution happens (the read-only-by-construction guard).
///
/// `server_engine` selects the engine variant: [`crate::mcp::ServerEngine::Fixture`]
/// for deterministic cluster-free CI, or [`crate::mcp::ServerEngine::Live`] for
/// a live `AtScale` endpoint.
///
/// # Errors
///
/// See [`PipelineError`] variants.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub fn run<S: std::hash::BuildHasher>(
    query: &Value,
    catalog: &Value,
    stats: &Value,
    tools: &ToolPaths,
    row_threshold: u64,
    server_engine: &crate::mcp::ServerEngine,
    backend_override: Option<&str>,
    capabilities: &BackendCapabilities,
    enriched_catalog_json: Option<&str>,
    xmla_model_coords: &HashMap<String, (String, String), S>,
    // Optional channel scope map from `ServerEnrichedData` for the
    // `ChannelScopeMismatch` guard (PRD-mqo-channel-scope-measure-grounding).
    // `None` disables the guard (conservative default for non-TPC-DS models).
    channel_scope_map: Option<&std::collections::BTreeMap<String, serde_json::Value>>,
) -> Result<PipelineOutput, PipelineError> {
    // ── Hard guard: input must be an MQO object, never a raw SQL string. ──
    if query.is_string() {
        return Err(PipelineError::NotAnMqo(
            "query_multidimensional accepts a Multidimensional Query Object, \
             not a raw SQL string; there is no SQL passthrough"
                .to_string(),
        ));
    }
    let mqo: mqo_spec::Mqo = serde_json::from_value(query.clone())
        .map_err(|e| PipelineError::NotAnMqo(e.to_string()))?;

    // Structural validation via mqo-spec.
    if let Err(errs) = mqo_spec::validate(&mqo) {
        let joined = errs
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        return Err(PipelineError::Invalid(joined));
    }

    // ── Param validation (pre-execution grounding guard) ───────────────────
    // After structural validation, before any subprocess: check that every
    // referenced field resolves against the catalog (unmapped measure/dimension/
    // filter, wrong hierarchy level, hand-rederived packaged calc). Conservative
    // by construction — when the validator returns no rejections the query
    // proceeds unchanged. When it rejects, we return a structured pre-execution
    // error naming the suggested entity and do NOT execute.
    if let Some(rejection) = param_validate(&mqo, catalog, channel_scope_map) {
        return Err(rejection);
    }

    // ── Calc-context pre-execution decline (PRD-mqo-calc-context-ratio-measures) ──
    // Packaged ratio/growth measures (is_calc=true in the catalog) require a
    // prior-period comparison context: either a time_intelligence op (YoY,
    // PriorPeriod) or a CalcGroupMember filter. Without it the measure returns
    // null — a silent failure that looks like data. Decline before execution so
    // the model can re-issue with the required context. Conservative: only fires
    // when the measure is *known* is_calc=true in the catalog; unknown measures
    // pass through (no false-positive for plain additive measures).
    if let Some(rejection) = check_calc_context(&mqo, catalog) {
        return Err(rejection);
    }

    let scratch = Scratch::new()?;
    let mqo_path = scratch.write(
        "mqo.json",
        &serde_json::to_string(&mqo).map_err(|e| PipelineError::Io(e.to_string()))?,
    )?;
    let catalog_path = scratch.write(
        "catalog.json",
        &serde_json::to_string(catalog).map_err(|e| PipelineError::Io(e.to_string()))?,
    )?;

    // Write enriched catalog to scratch dir when available (passed to bind step).
    let enriched_path: Option<PathBuf> = if let Some(json) = enriched_catalog_json {
        Some(scratch.write("enriched-catalog.json", json)?)
    } else {
        None
    };

    // ── Bind ─────────────────────────────────────────────────────────────
    let mut bound_value = bind_step(tools, &mqo_path, &catalog_path, enriched_path.as_deref())?;

    // ── Canonical near-twin labels (PRD-mqo-near-twin-dimension-drop, G2) ──
    // A near-twin dimension level (a role-playing/snowflaked copy whose caption
    // is the base caption prefixed with the relationship path, e.g.
    // `Promotion Product Item Item Product Name` for base `Item Product Name`)
    // would otherwise surface its **prefixed** caption as the result column,
    // breaking exact-name comparison against the canonical attribute. Attach the
    // canonical label to each bound dimension entry so `clean_result_rows`
    // emits the canonical name on the projected column. Pure label logic —
    // derived from the catalog's level-caption set, no domain metadata needed.
    attach_canonical_dimension_labels(&mut bound_value, catalog);

    let bound_path = scratch.write(
        "bound.json",
        &serde_json::to_string(&bound_value).map_err(|e| PipelineError::Io(e.to_string()))?,
    )?;

    // ── Route ────────────────────────────────────────────────────────────
    let stats_path = scratch.write(
        "stats.json",
        &serde_json::to_string(stats).map_err(|e| PipelineError::Io(e.to_string()))?,
    )?;
    // When forcing SQL, use threshold=0 so the router always emits sql_projection.
    let effective_threshold = if backend_override == Some("sql") { 0 } else { row_threshold };
    let decision = route_step(tools, &bound_path, &catalog_path, &stats_path, effective_threshold)?;

    let router_backend = backend_override
        .map(str::to_string)
        .or_else(|| {
            decision.get("backend").and_then(Value::as_str).map(str::to_string)
        })
        .unwrap_or_else(|| "dax".to_string());
    let estimated_rows = decision
        .get("estimated_rows")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let routing_reason = decision
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // ── Capability-based backend downgrade ───────────────────────────────
    let backend = apply_capability_downgrade(
        router_backend,
        backend_override,
        capabilities,
    )?;

    // ── Compile ──────────────────────────────────────────────────────────
    let compiled_query = compile_step(tools, &backend, &bound_path, &decision, &catalog_path)?;

    // ── SQL-string validation (ATSCALE-48466, multi-statement guard) ──────
    // Gate the compiled SQL before any warehouse round-trip. Only fires for the
    // `sql` backend; DAX/MDX pass through (their compilers produce non-SQL text).
    // Conservative: `validate_sql` is stateless and never false-positives on
    // well-formed single-SELECT output from the router's sql_projection.
    if backend == "sql" {
        let sql_violations = mqo_param_validator::sql_validator::validate_sql(&compiled_query);
        if !sql_violations.is_empty() {
            let count = sql_violations.len();
            let report = serde_json::to_value(&sql_violations)
                .unwrap_or_else(|_| serde_json::json!([]));
            return Err(PipelineError::SqlRejected { count, report });
        }
    }

    // ── Map backend string → enum ─────────────────────────────────────────
    let backend_enum = match backend.as_str() {
        "mdx" => Backend::Mdx,
        "sql" => Backend::Sql,
        _ => Backend::Dax,
    };

    // ── Filter bind report (always produced; empty when MQO has no filters) ──
    let (filters_applied, filters_dropped) = make_filter_report(&mqo.filters, &compiled_query);

    // ── Resolve XMLA model coordinate for DAX/MDX dispatch ───────────────
    // The executor's `parse_model_catalog_cube` requires a 3-segment path
    // `atscale_catalogs.<xmla_catalog>.<cube>`.  The MQO model field is a bare
    // cube name (e.g. `tpcds_benchmark_model`).  For DAX/MDX on the Live engine,
    // look up in the coordinate map (populated via discovery or static map).
    // For SQL and for the Fixture engine the bare model name is passed as-is —
    // the fixture engine ignores the model path entirely.
    let is_live = matches!(server_engine, crate::mcp::ServerEngine::Live(_));
    let resolved_model: String = if (backend_enum == Backend::Dax || backend_enum == Backend::Mdx) && is_live {
        match xmla_model_coords.get(mqo.model.as_str()) {
            Some((catalog_name, cube_name)) => {
                format!("atscale_catalogs.{catalog_name}.{cube_name}")
            }
            None => {
                return Err(PipelineError::XmlaCoordsNotFound {
                    model: mqo.model.clone(),
                });
            }
        }
    } else {
        mqo.model.clone()
    };

    // ── Execute ──────────────────────────────────────────────────────────
    let EngineResult { rows, row_cap_tripped } = match server_engine {
        crate::mcp::ServerEngine::Fixture => mqo_auth_bridge::Engine::execute(
            &FixtureEngine::with_bound(bound_value.clone()),
            &compiled_query,
            backend_enum,
            mqo.limit,
            Some(resolved_model.as_str()),
        )?,
        crate::mcp::ServerEngine::Live(ex) => {
            mqo_auth_bridge::Engine::execute(
                ex.as_ref(),
                &compiled_query,
                backend_enum,
                mqo.limit,
                Some(resolved_model.as_str()),
            )?
        }
    };

    // ── Result-completeness guard (PRD-mqo-null-path-result-guard-wiring) ──
    // A measure that is not materializable against the requested dimensions is
    // silently DROPPED by the DAX engine: `SUMMARIZECOLUMNS('t'[Net Profit Tier],
    // "Catalog Sales", [Catalog Sales])` returns dimension rows with NO measure
    // column. Returning those measure-less rows reads as a real answer (fm3-012).
    // Detect it catalog-independently: in a DAX result, measure columns are the
    // keys mangled as `[Measure]` → they start with `_x005b_`; dimension columns
    // are table-qualified (`atscale_catalogs_x005b_…`). If a non-empty result has
    // fewer measure columns than requested measures, ≥1 measure was dropped →
    // surface a typed cross-fact/non-materializable error instead of the rows.
    // (The group-based null-path detector can't catch this case: the enriched
    // catalog binds net_profit_tier to the catalog fact, so the groups intersect.)
    // Live DAX only: the fixture engine emits plain (non-XMLA-mangled) keys and
    // never drops measures, so the `_x005b_` column-shape test does not apply there.
    if is_live && backend == "dax" && !mqo.measures.is_empty() {
        if let Some(first) = rows.first().and_then(Value::as_object) {
            let measure_cols = dax_measure_column_count(first);
            if measure_cols < mqo.measures.len() {
                let report = serde_json::json!({
                    "null_path_incompatible": {
                        "reason": "the engine returned rows without all requested measure \
                                   columns — the measure(s) are not materializable against \
                                   the requested dimensions (cross-fact / non-materializable path)",
                        "requested_measures": mqo.measures.iter().map(|m| &m.unique_name).collect::<Vec<_>>(),
                        "measure_columns_returned": measure_cols,
                        "dimensions": mqo.dimensions.iter()
                            .map(|d| format!("{}.[{}]", d.hierarchy, d.level))
                            .collect::<Vec<_>>(),
                        "compiled_query": compiled_query,
                    }
                });
                return Err(PipelineError::CrossFactIncompatible { report });
            }
        }
    }

    // ── Dimension-completeness guard (PRD-mqo-near-twin-dimension-drop) ──
    // A dimension column can be silently DROPPED by the DAX engine when the
    // requested level is a "near-twin" — a label shared across ≥2 hierarchies
    // (e.g. `Product Category` exists on both `product_dimension` and
    // `promotion_product_item_product_dimension`). The engine returns
    // measure-only rows; without this guard the server would pass those rows
    // through and the model would mislabel the result.
    //
    // Detect it by counting NON-measure columns in the first result row.
    // Measure columns start with `_x005b_` (XMLA-mangled `[Measure Name]`);
    // every other column is a table-qualified dimension key
    // (`<table>_x005b_<Level>_x005d_`). If fewer non-measure columns are
    // returned than requested dimensions, at least one dimension was dropped.
    //
    // Live DAX only — mirrors the measure guard's `is_live && backend == "dax"`
    // gate (fixture engine emits plain keys and never drops columns).
    if is_live && backend == "dax" && !mqo.dimensions.is_empty() {
        if let Some(first) = rows.first().and_then(Value::as_object) {
            let dim_cols = dax_dim_column_count(first);
            if dim_cols < mqo.dimensions.len() {
                let missing = mqo.dimensions.len() - dim_cols;
                let dimension_columns_returned: Vec<&String> =
                    first.keys().filter(|k| !k.starts_with("_x005b_")).collect();
                let report = serde_json::json!({
                    "dimension_not_materialized": {
                        "reason": "the engine returned rows without all requested dimension \
                                   columns — a near-twin level (shared across hierarchies) or \
                                   other catalog-ingest-state-dependent column drop",
                        "requested_dimensions": mqo.dimensions.iter()
                            .map(|d| format!("{}.[{}]", d.hierarchy, d.level))
                            .collect::<Vec<_>>(),
                        "dimension_columns_returned": dimension_columns_returned,
                        "compiled_query": compiled_query,
                    }
                });
                return Err(PipelineError::DimensionNotMaterialized {
                    missing,
                    requested: mqo.dimensions.len(),
                    report,
                });
            }
        }
    }

    Ok(PipelineOutput {
        backend,
        estimated_rows,
        routing_reason,
        compiled_query,
        rows,
        bound: bound_value,
        filters_applied,
        filters_dropped,
        row_cap_tripped,
    })
}

/// Apply the capability probe's downgrade policy to the router-selected backend.
///
/// - When `backend_override` is `Some`, the backend is forced — no downgrade.
/// - Otherwise, consult `capabilities.effective_backend`:
///   - If the effective backend matches the requested one → no change.
///   - If downgraded to SQL → log and return `"sql"`.
///   - If `effective_backend` returns `None` → all backends dead → error.
fn apply_capability_downgrade(
    router_backend: String,
    backend_override: Option<&str>,
    capabilities: &BackendCapabilities,
) -> Result<String, PipelineError> {
    if backend_override.is_some() {
        // Forced backend — respect unconditionally, no capability check.
        return Ok(router_backend);
    }

    let requested_enum = match router_backend.as_str() {
        "mdx" => Backend::Mdx,
        "sql" => Backend::Sql,
        _ => Backend::Dax,
    };

    match capabilities.effective_backend(requested_enum) {
        Some(effective) if effective == requested_enum => Ok(router_backend),
        Some(Backend::Sql) => {
            eprintln!(
                "mqo-mcp-server: downgraded: {}→sql ({})",
                router_backend,
                capabilities.downgrade_reason(requested_enum)
            );
            Ok("sql".to_string())
        }
        Some(other) => {
            let s = match other {
                Backend::Dax => "dax",
                Backend::Mdx => "mdx",
                Backend::Sql => "sql",
            };
            Ok(s.to_string())
        }
        None => Err(PipelineError::NoBackendAvailable {
            dax: capabilities.dax.to_string(),
            mdx: capabilities.mdx.to_string(),
            sql: capabilities.sql.to_string(),
        }),
    }
}

/// Run the pre-execution param-validator over the MQO and catalog snapshot.
///
/// Builds the validator's `CatalogSnapshot` from the server catalog columns and
/// a `BoundMqoInput` from the submitted MQO, then calls
/// [`mqo_param_validator::validate`]. Returns `Some(PipelineError::ParamRejected)`
/// when the validator reports ≥1 rejection, `None` otherwise.
///
/// # Conservative construction (zero false positives on valid queries)
///
/// - **Measures**: each catalog measure is registered under both its
///   `unique_name` and its `label` (callers commonly reference the label, e.g.
///   `"Revenue"` for `sales.revenue`), so a label-based reference resolves.
/// - **Dimensions**: registered by hierarchy name (the MQO `LevelSelection`
///   uses `hierarchy` as its dimension key).
/// - **Hierarchies**: one per hierarchy, with its level *labels* — so the
///   wrong-hierarchy-level check matches the MQO's `level` (a label).
/// - **`date_roles`** and measure `subject_area` are left empty/None, which
///   disables the AmbiguousDateRole and CrossFactPath passes (the binder's
///   `bind_with_date_roles` / cross-fact check own those concerns). This keeps
///   the validator focused on unmapped fields, wrong levels, and packaged-calc
///   re-derivation without duplicating the binder's job or risking false
///   positives on conformed dims.
fn param_validate(
    mqo: &mqo_spec::Mqo,
    catalog: &Value,
    // Optional per-measure channel scope map from `ServerEnrichedData.channel_scope_map`
    // (PRD-mqo-channel-scope-measure-grounding, FR3). When `None`, the
    // `ChannelScopeMismatch` guard stays dormant (no false positives on
    // unenriched catalogs — OQ4 conservative behavior).
    channel_scope_map: Option<&std::collections::BTreeMap<String, serde_json::Value>>,
) -> Option<PipelineError> {
    use mqo_param_validator::{
        validate, BoundMqoInput, CatalogHierarchy, CatalogMeasure, CatalogSnapshot,
        LevelDomainMeta, LevelValueType, MqoDimensionRef, MqoFilterRef, MqoMeasureRef,
    };
    use std::collections::BTreeMap;

    let cols = catalog.get("columns").and_then(Value::as_array)?;

    let mut measures: Vec<CatalogMeasure> = Vec::new();
    // hierarchy -> set of level labels (preserve first-seen order via Vec + dedup)
    let mut hier_levels: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // hierarchy -> per-level domain/type metadata (Rule 4 filter-level guard).
    // Populated from `value_type`/`domain`/`expected_key_shape` on level columns
    // (written by the catalog-capture probe, tools/capture_level_meta.py).
    let mut hier_meta: BTreeMap<String, Vec<LevelDomainMeta>> = BTreeMap::new();

    for c in cols {
        let kind = c.get("kind").and_then(Value::as_str).unwrap_or("");
        match kind {
            "measure" => {
                let un = c.get("unique_name").and_then(Value::as_str).unwrap_or("");
                if un.is_empty() {
                    continue;
                }
                let label = c.get("label").and_then(Value::as_str).map(str::to_string);
                let is_calc = c.get("is_calc").and_then(Value::as_bool);
                // Carry the source `semi_additive` flag into the validator's
                // snapshot so RULE 2 (semi-additive sum guard) is no longer
                // dormant. The catalog-binder `ColumnEntry` exposes it as a
                // `SemiAdditiveInfo` object (present ⇒ semi-additive) in live
                // mode; a plain bool is also accepted. Absent/null ⇒ None.
                let semi_additive = match c.get("semi_additive") {
                    Some(Value::Bool(b)) => Some(*b),
                    Some(Value::Object(_)) => Some(true),
                    _ => None,
                };
                // Derive channel_scope from the map when available
                // (PRD-mqo-channel-scope-measure-grounding, FR3).
                let channel_scope: Option<Vec<String>> = channel_scope_map
                    .and_then(|m| m.get(un))
                    .and_then(|v| v.get("channel_groups"))
                    .and_then(serde_json::Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    });
                // Primary entry under unique_name.
                measures.push(CatalogMeasure {
                    unique_name: un.to_string(),
                    subject_area: None,
                    label: label.clone(),
                    is_calc,
                    semi_additive,
                    channel_scope: channel_scope.clone(),
                    ..Default::default()
                });
                // Alias entry under label when it differs (callers reference the
                // display label, e.g. "Revenue" for "sales.revenue").
                if let Some(ref l) = label {
                    if l != un {
                        measures.push(CatalogMeasure {
                            unique_name: l.clone(),
                            subject_area: None,
                            label: Some(l.clone()),
                            is_calc,
                            semi_additive,
                            channel_scope: channel_scope.clone(),
                            ..Default::default()
                        });
                    }
                }
            }
            "level" => {
                let hier = c
                    .get("hierarchy")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        c.get("unique_name")
                            .and_then(Value::as_str)
                            .and_then(|un| un.split_once('.').map(|(h, _)| h.to_string()))
                    });
                let level_label = c
                    .get("level")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| c.get("label").and_then(Value::as_str).map(str::to_string));
                if let (Some(h), Some(lvl)) = (hier, level_label) {
                    // Rule 4 metadata: carry the level's value type + (bounded)
                    // member domain / key-shape when the catalog column has it.
                    if let Some(vt) = c.get("value_type").and_then(Value::as_str) {
                        let value_type = match vt {
                            "integer" => LevelValueType::Integer,
                            "date" => LevelValueType::Date,
                            "decimal" => LevelValueType::Decimal,
                            _ => LevelValueType::String,
                        };
                        let domain = c.get("domain").and_then(Value::as_array).map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect::<Vec<_>>()
                        });
                        let expected_key_shape = c
                            .get("expected_key_shape")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        hier_meta.entry(h.clone()).or_default().push(LevelDomainMeta {
                            level: lvl.clone(),
                            value_type,
                            domain,
                            expected_key_shape,
                        });
                    }
                    let entry = hier_levels.entry(h).or_default();
                    if !entry.contains(&lvl) {
                        entry.push(lvl);
                    }
                }
            }
            _ => {}
        }
    }

    let dimensions = hier_levels
        .keys()
        .map(|h| mqo_param_validator::CatalogDimension {
            unique_name: h.clone(),
            subject_areas: Vec::new(),
        })
        .collect();
    let hierarchies: Vec<CatalogHierarchy> = hier_levels
        .iter()
        .map(|(h, levels)| CatalogHierarchy {
            dimension_unique_name: h.clone(),
            hierarchy_unique_name: h.clone(),
            levels: levels.clone(),
            level_meta: hier_meta.get(h).cloned().unwrap_or_default(),
            fact_local_facts: vec![],
        })
        .collect();

    let snapshot = CatalogSnapshot {
        measures,
        dimensions,
        hierarchies,
        date_roles: Vec::new(),
    };

    // Build the bound-MQO view the validator consumes.
    let input = BoundMqoInput {
        measures: mqo
            .measures
            .iter()
            .map(|m| MqoMeasureRef {
                unique_name: m.unique_name.clone(),
                ..Default::default()
            })
            .collect(),
        dimensions: mqo
            .dimensions
            .iter()
            .map(|d| MqoDimensionRef {
                unique_name: d.hierarchy.clone(),
                level: Some(d.level.clone()),
                hierarchy: Some(d.hierarchy.clone()),
                role_qualifier: None,
            })
            .collect(),
        filters: mqo
            .filters
            .iter()
            .filter_map(|f| match f {
                // Member filters carry the member keys but no level (mqo_spec
                // `Member` is hierarchy + members). The validator's member-domain
                // check (Rule 4) compares them against the hierarchy's enumerated
                // level domains, conservatively.
                mqo_spec::Filter::Member { hierarchy, members } => Some(MqoFilterRef {
                    unique_name: hierarchy.clone(),
                    level: None,
                    members: members.clone(),
                    ..Default::default()
                }),
                // Range filters carry a level unique_name ("hier.[Label]") + bounds.
                // Only attach the level when we actually have level_meta for it, so
                // the rule runs value-fit; otherwise leave level None (rule skips) —
                // this prevents a false "level does not exist" rejection from a
                // label-format mismatch on the common year-range filters.
                mqo_spec::Filter::Range { level, lo, hi } => {
                    let (hier, label) = match level.split_once(".[") {
                        Some((h, rest)) => {
                            (h.to_string(), rest.trim_end_matches(']').to_string())
                        }
                        None => (String::new(), level.clone()),
                    };
                    let has_meta = hier_meta
                        .get(&hier)
                        .map(|v| v.iter().any(|m| m.level == label))
                        .unwrap_or(false);
                    let bound_str = |b: &mqo_spec::RangeBound| {
                        if let Some(n) = b.as_f64() {
                            if n.fract() == 0.0 { (n as i64).to_string() } else { n.to_string() }
                        } else {
                            b.as_str().unwrap_or("").to_string()
                        }
                    };
                    Some(MqoFilterRef {
                        unique_name: hier,
                        level: if has_meta { Some(label) } else { None },
                        range_lo: Some(bound_str(lo)),
                        range_hi: Some(bound_str(hi)),
                        ..Default::default()
                    })
                }
                _ => None,
            })
            .collect(),
    };

    let mut rejections = validate(&input, &snapshot);

    // Drop `Unmapped` rejections: unknown measure/dimension/filter references are
    // the *binder's* responsibility — it returns a richer `not_found` report
    // (exit 4) that the pipeline already surfaces as `PipelineError::NotGround`.
    // Letting the validator pre-empt that would change the error code for the
    // not-found path (a regression) and duplicate work. The validator's unique
    // value over the binder is the *grounded-but-wrong* checks:
    // WrongHierarchyLevel (right concept, wrong hierarchy) and
    // ManualCalcRederivation (hand-rolled period-over-period that a packaged calc
    // already provides) — those carry `suggested_calc` / nearest-match hints the
    // binder does not produce.
    use mqo_param_validator::RejectReason;
    rejections.retain(|r| r.reason != RejectReason::Unmapped);

    if rejections.is_empty() {
        return None;
    }
    let report = serde_json::to_value(&rejections)
        .map(|r| serde_json::json!({ "param_rejections": r }))
        .unwrap_or_else(|_| serde_json::json!({ "param_rejections": [] }));
    Some(PipelineError::ParamRejected {
        count: rejections.len(),
        report,
    })
}

/// Pre-execution calc-context check (PRD-mqo-calc-context-ratio-measures).
///
/// Returns `Some(PipelineError::ParamRejected)` when the MQO references one or
/// more measures that are marked `is_calc=true` in the catalog AND no
/// time-intelligence context is present (empty `time_intelligence` array AND
/// no `CalcGroupMember` filter). Conservative: measures not found in the
/// catalog, or whose `is_calc` field is absent/false, pass through unchanged.
///
/// Rejected measures are listed by name in the structured `param_rejections`
/// report with `"reason": "missing_calc_context"` so the model can re-issue
/// with a `time_intelligence` op (e.g. `YoY`, `PriorPeriod`) or a
/// `CalcGroupMember` filter.
fn check_calc_context(mqo: &mqo_spec::Mqo, catalog: &Value) -> Option<PipelineError> {
    // If the MQO already carries time-intelligence context, pass through.
    if !mqo.time_intelligence.is_empty() {
        return None;
    }
    // If a CalcGroupMember filter is present, that provides the comparison context.
    let has_calc_group = mqo.filters.iter().any(|f| {
        matches!(f, mqo_spec::Filter::CalcGroupMember { .. })
    });
    if has_calc_group {
        return None;
    }

    let cols = catalog.get("columns").and_then(Value::as_array)?;

    // Build a set of unique_names (and labels) that are is_calc=true in the catalog.
    // We check both unique_name and label so that label-based MQO references are caught.
    use std::collections::HashSet;
    let mut calc_unique_names: HashSet<String> = HashSet::new();
    for col in cols {
        let kind = col.get("kind").and_then(Value::as_str).unwrap_or("");
        if kind != "measure" {
            continue;
        }
        let is_calc = col.get("is_calc").and_then(Value::as_bool).unwrap_or(false);
        if !is_calc {
            continue;
        }
        if let Some(un) = col.get("unique_name").and_then(Value::as_str) {
            calc_unique_names.insert(un.to_string());
        }
        if let Some(lbl) = col.get("label").and_then(Value::as_str) {
            calc_unique_names.insert(lbl.to_string());
        }
    }

    // Fallback: AtScale XMLA does not always set is_calc=true for packaged
    // ratio/growth calc members (the live model returns is_calc=false for
    // "Web Sales Increase", "Store Sales Increase", etc.). Supplement with
    // a label-pattern heuristic: any measure whose label contains one of
    // these context-implying keywords almost certainly needs a prior-period
    // frame. OQ-2 resolution: curated-pattern approach until is_calc is
    // faithfully captured in the catalog snapshot.
    const CALC_KEYWORDS: &[&str] = &[
        "increase", "growth", "ratio", "change", "prior period",
        "yoy", "vs prior", "vs last", "week over week", "mom", "qoq",
    ];
    for col in cols {
        let kind = col.get("kind").and_then(Value::as_str).unwrap_or("");
        if kind != "measure" { continue; }
        let lbl = col.get("label").and_then(Value::as_str).unwrap_or("").to_lowercase();
        if CALC_KEYWORDS.iter().any(|k| lbl.contains(k)) {
            if let Some(un) = col.get("unique_name").and_then(Value::as_str) {
                calc_unique_names.insert(un.to_string());
            }
            if let Some(l) = col.get("label").and_then(Value::as_str) {
                calc_unique_names.insert(l.to_string());
            }
        }
    }

    if calc_unique_names.is_empty() {
        return None;
    }

    // Find which MQO measures reference a known is_calc measure.
    let offending: Vec<String> = mqo
        .measures
        .iter()
        .filter(|m| calc_unique_names.contains(&m.unique_name))
        .map(|m| m.unique_name.clone())
        .collect();

    if offending.is_empty() {
        return None;
    }

    let rejections: Vec<serde_json::Value> = offending
        .iter()
        .map(|name| {
            serde_json::json!({
                "field": name,
                "reason": "missing_calc_context",
                "message": format!(
                    "Packaged calc measure '{}' is a ratio/growth member that requires a \
                     prior-period comparison context to produce a non-null result.",
                    name
                ),
                "hint": "Add a time_intelligence op (e.g. YoY, PriorPeriod) or a \
                         CalcGroupMember filter to provide the comparison period. Without \
                         this context the measure evaluates to null."
            })
        })
        .collect();

    let report = serde_json::json!({ "param_rejections": rejections });
    Some(PipelineError::ParamRejected {
        count: offending.len(),
        report,
    })
}

/// Produce `filters_applied` and `filters_dropped` arrays from the MQO's Member filters.
///
/// Uses the `mqoguard-filter-bind-report` heuristic: each filter's member keys are searched
/// for in the compiled SQL text. Non-Member filters (`Range`, `CalcGroupMember`) are not
/// represented in `mqoguard_filter_bind_report::MqoFilter` and are skipped.
fn make_filter_report(
    mqo_filters: &[mqo_spec::Filter],
    compiled_sql: &str,
) -> (Vec<Value>, Vec<Value>) {
    let fr_filters: Vec<MqoFilter> = mqo_filters
        .iter()
        .enumerate()
        .filter_map(|(i, f)| match f {
            mqo_spec::Filter::Member { hierarchy, members } => {
                Some(MqoFilter::Member(MemberFilter {
                    filter_id: i.to_string(),
                    hierarchy: hierarchy.clone(),
                    level: None,
                    members: members.clone(),
                }))
            }
            _ => None,
        })
        .collect();

    let bound_mqo = BoundMqo {
        filters: fr_filters,
        catalog: None,
    };
    let compiled = CompiledQuery {
        sql: compiled_sql.to_string(),
        bound_filter_ids: None,
    };

    let report = report_filters(&bound_mqo, &compiled);

    let applied = report
        .applied
        .iter()
        .filter_map(|a| serde_json::to_value(a).ok())
        .collect();
    let dropped = report
        .dropped
        .iter()
        .filter_map(|d| serde_json::to_value(d).ok())
        .collect();

    (applied, dropped)
}

/// Run `mqo-bind`; return its `BoundMqo` JSON, or the not-found / ambiguous / incompatible
/// report as the appropriate [`PipelineError`] variant.
///
/// When `enriched_catalog_path` is `Some`, passes `--enriched-catalog <path>` so the binder's
/// `bind_with_compat` path activates and can return exit 5 (`CrossFactIncompatible`).
/// Collect every dimension-level **caption** in the catalog into a set.
///
/// Used as the registry the near-twin canonical-suffix derivation
/// (`canonical_level_label`) checks against. Captions are read from each
/// `kind == "level"` column's `level` (falling back to `label`).
fn catalog_level_captions(catalog: &Value) -> std::collections::HashSet<String> {
    let mut captions = std::collections::HashSet::new();
    if let Some(cols) = catalog.get("columns").and_then(Value::as_array) {
        for c in cols {
            if c.get("kind").and_then(Value::as_str) != Some("level") {
                continue;
            }
            let caption = c
                .get("level")
                .and_then(Value::as_str)
                .or_else(|| c.get("label").and_then(Value::as_str));
            if let Some(cap) = caption {
                captions.insert(cap.to_string());
            }
        }
    }
    captions
}

/// Attach a canonical `label` to each bound **dimension** entry whose level is a
/// near-twin (PRD-mqo-near-twin-dimension-drop, G2 — canonical output labels).
///
/// For each `bound.dimensions[i]`, the level caption is taken from its
/// `unique_name` (`hier.[Level Caption]`); `canonical_level_label` collapses a
/// near-twin caption to the shared base attribute name. When the canonical label
/// differs from the verbatim caption, it is written as the entry's `label`,
/// which `clean_result_rows` prefers when naming the result column. Unique
/// (non-twin) levels are left untouched (no `label` added).
fn attach_canonical_dimension_labels(bound: &mut Value, catalog: &Value) {
    let captions = catalog_level_captions(catalog);
    let Some(dims) = bound.get_mut("dimensions").and_then(Value::as_array_mut) else {
        return;
    };
    for dim in dims {
        let Some(un) = dim.get("unique_name").and_then(Value::as_str) else {
            continue;
        };
        // The level caption is the contents of the trailing `[...]` of the
        // unique_name (`hier.[Caption]`); fall back to the part after the dot.
        let caption = un
            .rsplit_once(".[")
            .map(|(_, rest)| rest.trim_end_matches(']').to_string())
            .or_else(|| un.split_once('.').map(|(_, c)| c.to_string()))
            .unwrap_or_else(|| un.to_string());
        let canonical = crate::handle_ops::canonical_level_label(&caption, &captions);
        if canonical != caption {
            if let Some(obj) = dim.as_object_mut() {
                obj.insert("label".to_string(), Value::String(canonical));
            }
        }
    }
}

fn bind_step(
    tools: &ToolPaths,
    mqo_path: &Path,
    catalog_path: &Path,
    enriched_catalog_path: Option<&Path>,
) -> Result<Value, PipelineError> {
    // Build args as owned OsStrings so we can hold references into them.
    let mut args_os: Vec<std::ffi::OsString> = vec![
        "--mqo".into(),
        mqo_path.as_os_str().to_owned(),
        "--catalog".into(),
        catalog_path.as_os_str().to_owned(),
    ];
    if let Some(ep) = enriched_catalog_path {
        args_os.push("--enriched-catalog".into());
        args_os.push(ep.as_os_str().to_owned());
    }
    let args_ref: Vec<&std::ffi::OsStr> = args_os.iter().map(std::ffi::OsString::as_os_str).collect();

    let out = run_tool(&tools.bind, &args_ref, "mqo-bind")?;
    match out.code {
        0 => serde_json::from_str(&out.stdout).map_err(|e| PipelineError::Subprocess {
            tool: "mqo-bind".to_string(),
            detail: format!("binder stdout was not JSON: {e}"),
        }),
        3 | 4 => {
            let report: Value = serde_json::from_str(&out.stdout)
                .unwrap_or_else(|_| serde_json::json!({ "binder_error": out.stdout }));
            Err(PipelineError::NotGround { report })
        }
        5 => {
            let report: Value = serde_json::from_str(&out.stdout)
                .unwrap_or_else(|_| serde_json::json!({ "binder_error": out.stdout }));
            Err(PipelineError::CrossFactIncompatible { report })
        }
        other => Err(PipelineError::Subprocess {
            tool: "mqo-bind".to_string(),
            detail: format!("unexpected exit code {other}: {}", out.stderr),
        }),
    }
}

/// Run `mqo-route`; return the routing-decision JSON.
///
/// `catalog_path` is forwarded as `--catalog` so the router can build
/// fully-qualified, display-label SQL projections.
fn route_step(
    tools: &ToolPaths,
    bound_path: &Path,
    catalog_path: &Path,
    stats_path: &Path,
    row_threshold: u64,
) -> Result<Value, PipelineError> {
    let threshold = row_threshold.to_string();
    let out = run_tool(
        &tools.route,
        &[
            "--bound".as_ref(),
            bound_path.as_os_str(),
            "--catalog".as_ref(),
            catalog_path.as_os_str(),
            "--stats".as_ref(),
            stats_path.as_os_str(),
            "--row-threshold".as_ref(),
            threshold.as_ref(),
        ],
        "mqo-route",
    )?;
    if out.code != 0 {
        return Err(PipelineError::Subprocess {
            tool: "mqo-route".to_string(),
            detail: format!("exit {}: {}", out.code, out.stderr),
        });
    }
    serde_json::from_str(&out.stdout).map_err(|e| PipelineError::Subprocess {
        tool: "mqo-route".to_string(),
        detail: format!("router stdout was not JSON: {e}"),
    })
}

/// Compile the bound MQO for the chosen backend. DAX/MDX shell out to their
/// compilers; SQL uses the router's `sql_projection`.
fn compile_step(
    tools: &ToolPaths,
    backend: &str,
    bound_path: &Path,
    decision: &Value,
    catalog_path: &Path,
) -> Result<String, PipelineError> {
    match backend {
        "dax" => compile_with(&tools.dax, bound_path, Some(catalog_path), "mqo-dax"),
        "mdx" => compile_with(&tools.mdx, bound_path, None, "mqo-mdx"),
        "sql" => Ok(decision
            .get("sql_projection")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()),
        other => Err(PipelineError::Subprocess {
            tool: "mqo-route".to_string(),
            detail: format!("router returned unknown backend `{other}`"),
        }),
    }
}

fn compile_with(bin: &Path, bound_path: &Path, catalog_path: Option<&Path>, name: &str) -> Result<String, PipelineError> {
    let mut args: Vec<&std::ffi::OsStr> = vec!["--bound".as_ref(), bound_path.as_os_str()];
    if let Some(cp) = catalog_path {
        args.push("--catalog".as_ref());
        args.push(cp.as_os_str());
    }
    let out = run_tool(bin, &args, name)?;
    if out.code != 0 {
        return Err(PipelineError::Subprocess {
            tool: name.to_string(),
            detail: format!("exit {}: {}", out.code, out.stderr),
        });
    }
    Ok(out.stdout.trim_end().to_string())
}

/// Captured output of a subprocess run.
struct ToolOut {
    code: i32,
    stdout: String,
    stderr: String,
}

fn run_tool(
    bin: &Path,
    args: &[&std::ffi::OsStr],
    tool_name: &str,
) -> Result<ToolOut, PipelineError> {
    let output = Command::new(bin)
        .args(args)
        .output()
        .map_err(|e| PipelineError::Subprocess {
            tool: tool_name.to_string(),
            detail: format!("could not launch {}: {e}", bin.display()),
        })?;
    Ok(ToolOut {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Count DAX measure columns in a result row. In a live DAX `SUMMARIZECOLUMNS`
/// result, measure columns are mangled `[Measure]` → keys start with `_x005b_`;
/// dimension columns are table-qualified (`atscale_catalogs_x005b_…`). A
/// non-materializable measure is silently dropped, so this count drops below the
/// number of requested measures — the signal the result-completeness guard uses.
fn dax_measure_column_count(row: &serde_json::Map<String, Value>) -> usize {
    row.keys().filter(|k| k.starts_with("_x005b_")).count()
}

/// Count DAX dimension columns in a result row. In a live DAX result, dimension
/// columns are table-qualified keys that do NOT start with `_x005b_` (the
/// measure prefix). A silently-dropped dimension (near-twin column drop) is
/// detected when this count falls below the number of requested dimensions.
fn dax_dim_column_count(row: &serde_json::Map<String, Value>) -> usize {
    row.keys().filter(|k| !k.starts_with("_x005b_")).count()
}

#[cfg(test)]
mod result_guard_tests {
    use super::{dax_measure_column_count, dax_dim_column_count, PipelineError};
    use serde_json::json;

    #[test]
    fn counts_measure_columns_not_dimensions() {
        // Healthy row: one measure ([Catalog Sales]) + one dimension (table[Product Category]).
        let row = json!({
            "_x005b_Catalog_x0020_Sales_x005d_": 123.0,
            "atscale_catalogs_x005b_Product_x0020_Category_x005d_": "Books"
        });
        assert_eq!(dax_measure_column_count(row.as_object().unwrap()), 1);
    }

    #[test]
    fn dropped_measure_yields_zero() {
        // fm3-012 shape: the measure column was dropped; only the dimension remains.
        let row = json!({
            "atscale_catalogs_x005b_Net_x0020_Profit_x0020_Tier_x005d_": "300-2000"
        });
        assert_eq!(dax_measure_column_count(row.as_object().unwrap()), 0);
    }

    // ── Dimension-completeness guard tests (PRD-mqo-near-twin-dimension-drop) ──

    #[test]
    fn counts_dim_columns_not_measures() {
        // Healthy row: one measure + one dimension. Only the dimension key should be counted.
        let row = json!({
            "_x005b_Total_x0020_Product_x0020_Count_x005d_": 482.0,
            "product_dimension_x005b_Product_x0020_Category_x005d_": "Books"
        });
        assert_eq!(dax_dim_column_count(row.as_object().unwrap()), 1);
    }

    #[test]
    fn near_twin_drop_yields_zero_dim_cols() {
        // Near-twin drop shape (live repro v0.33.0): only the measure column was returned;
        // Product Category was dropped — the dimension-completeness guard fires on this.
        let row = json!({
            "_x005b_Total_x0020_Product_x0020_Count_x005d_": 482.0
        });
        assert_eq!(dax_dim_column_count(row.as_object().unwrap()), 0);
    }

    #[test]
    fn dim_guard_all_dims_present_passes() {
        // Simulate DimensionNotMaterialized check: enough dim columns → guard does NOT fire.
        // Two requested dimensions, two non-measure columns in result.
        let row = json!({
            "_x005b_Total_x0020_Product_x0020_Count_x005d_": 100.0,
            "product_dimension_x005b_Product_x0020_Category_x005d_": "Books",
            "product_dimension_x005b_Brand_x0020_Name_x005d_": "Acme"
        });
        let obj = row.as_object().unwrap();
        let dim_cols = dax_dim_column_count(obj);
        let requested = 2usize;
        // Guard condition: dim_cols < requested → error. Here they are equal → no error.
        assert!(dim_cols >= requested, "all dims present, guard should not fire");
    }

    #[test]
    fn dim_guard_missing_dim_returns_dimension_not_materialized() {
        // Simulate the guard logic inline: one dimension requested but zero dim columns
        // returned → the guard should yield DimensionNotMaterialized.
        use mqo_spec::{Mqo, MeasureRef, LevelSelection};
        let mqo = Mqo {
            model: "tpcds".to_string(),
            measures: vec![MeasureRef { unique_name: "tpcds.total_product_count".to_string() }],
            dimensions: vec![LevelSelection {
                hierarchy: "product_dimension".to_string(),
                level: "Product Category".to_string(),
            }],
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
            projection: false,
        };
        // Only the measure column is present — dimension was dropped (near-twin drop shape).
        let row = json!({
            "_x005b_Total_x0020_Product_x0020_Count_x005d_": 482.0
        });
        let first = row.as_object().unwrap();
        let dim_cols = dax_dim_column_count(first);
        assert!(
            dim_cols < mqo.dimensions.len(),
            "dim_cols={dim_cols} should be less than requested={}",
            mqo.dimensions.len()
        );
        // Verify the error variant construction is well-formed.
        let missing = mqo.dimensions.len() - dim_cols;
        let dimension_columns_returned: Vec<&String> =
            first.keys().filter(|k| !k.starts_with("_x005b_")).collect();
        let report = serde_json::json!({
            "dimension_not_materialized": {
                "reason": "the engine returned rows without all requested dimension columns",
                "requested_dimensions": mqo.dimensions.iter()
                    .map(|d| format!("{}.[{}]", d.hierarchy, d.level))
                    .collect::<Vec<_>>(),
                "dimension_columns_returned": dimension_columns_returned,
                "compiled_query": "SUMMARIZECOLUMNS('product_dimension'[Product Category], \"Total Product Count\", [Total Product Count])",
            }
        });
        let err = PipelineError::DimensionNotMaterialized {
            missing,
            requested: mqo.dimensions.len(),
            report,
        };
        // Verify the error message renders correctly and the fields are populated.
        let msg = err.to_string();
        assert!(msg.contains("dimension_not_materialized"), "error message: {msg}");
        assert!(msg.contains("1 of 1"), "error message should contain counts: {msg}");
        if let PipelineError::DimensionNotMaterialized { missing, requested, .. } = err {
            assert_eq!(missing, 1);
            assert_eq!(requested, 1);
        }
    }
}

#[cfg(test)]
mod canonical_label_tests {
    use super::{attach_canonical_dimension_labels, catalog_level_captions};
    use serde_json::{json, Value};

    /// Minimal catalog with a base hierarchy + two near-twin hierarchies (the
    /// TPC-DS benchmark-model shape that drives both eval failures).
    fn near_twin_catalog() -> Value {
        json!({
            "catalog": "atscale_catalogs",
            "columns": [
                { "kind": "level", "hierarchy": "product_dimension",
                  "unique_name": "product_dimension.[Item Product Name]", "level": "Item Product Name" },
                { "kind": "level", "hierarchy": "product_dimension",
                  "unique_name": "product_dimension.[Product Brand Name]", "level": "Product Brand Name" },
                { "kind": "level", "hierarchy": "product_dimension",
                  "unique_name": "product_dimension.[Product Category]", "level": "Product Category" },
                { "kind": "level", "hierarchy": "promotion_product_item_product_dimension",
                  "unique_name": "promotion_product_item_product_dimension.[Promotion Product Item Item Product Name]",
                  "level": "Promotion Product Item Item Product Name" },
                { "kind": "level", "hierarchy": "store_item_product_dimension",
                  "unique_name": "store_item_product_dimension.[Store Item Product Category]",
                  "level": "Store Item Product Category" },
                { "kind": "level", "hierarchy": "sold_date_dimensions",
                  "unique_name": "sold_date_dimensions.[Sold Calendar Year]", "level": "Sold Calendar Year" },
                { "kind": "measure", "unique_name": "tpcds_benchmark_model.total_product_count",
                  "label": "Total Product Count" }
            ]
        })
    }

    #[test]
    fn level_captions_collects_all_levels() {
        let caps = catalog_level_captions(&near_twin_catalog());
        assert!(caps.contains("Item Product Name"));
        assert!(caps.contains("Promotion Product Item Item Product Name"));
        assert!(caps.contains("Sold Calendar Year"));
        // Measures are NOT level captions.
        assert!(!caps.contains("Total Product Count"));
    }

    /// Failure-1 shape: a near-twin dimension entry gets the canonical `label`
    /// (`Item Product Name`) attached; unique levels get none.
    #[test]
    fn attaches_canonical_label_to_near_twin_dimension() {
        let catalog = near_twin_catalog();
        // Bound shape as emitted by the binder (no label on dimensions).
        let mut bound = json!({
            "measures": [{ "unique_name": "tpcds_benchmark_model.total_product_count" }],
            "dimensions": [
                { "unique_name": "promotion_product_item_product_dimension.[Promotion Product Item Item Product Name]",
                  "hierarchy": "promotion_product_item_product_dimension" }
            ]
        });
        attach_canonical_dimension_labels(&mut bound, &catalog);
        let dim = &bound["dimensions"][0];
        assert_eq!(dim["label"], json!("Item Product Name"));
    }

    /// Failure-2 shape: the store-prefixed near-twin category collapses to the
    /// canonical `Product Category`.
    #[test]
    fn attaches_canonical_label_for_store_twin_category() {
        let catalog = near_twin_catalog();
        let mut bound = json!({
            "measures": [{ "unique_name": "tpcds_benchmark_model.total_product_count" }],
            "dimensions": [
                { "unique_name": "store_item_product_dimension.[Store Item Product Category]",
                  "hierarchy": "store_item_product_dimension" }
            ]
        });
        attach_canonical_dimension_labels(&mut bound, &catalog);
        assert_eq!(bound["dimensions"][0]["label"], json!("Product Category"));
    }

    /// FR-4 (no regression): a base / unique level gets NO `label` added — its
    /// caption is already canonical, so the column name is unchanged.
    #[test]
    fn base_and_unique_levels_unchanged() {
        let catalog = near_twin_catalog();
        let mut bound = json!({
            "measures": [{ "unique_name": "tpcds_benchmark_model.total_product_count" }],
            "dimensions": [
                { "unique_name": "product_dimension.[Product Category]",
                  "hierarchy": "product_dimension" },
                { "unique_name": "sold_date_dimensions.[Sold Calendar Year]",
                  "hierarchy": "sold_date_dimensions" }
            ]
        });
        attach_canonical_dimension_labels(&mut bound, &catalog);
        // No canonical collapse → no `label` injected.
        assert!(bound["dimensions"][0].get("label").is_none());
        assert!(bound["dimensions"][1].get("label").is_none());
    }
}

#[cfg(test)]
mod calc_context_tests {
    use super::{check_calc_context, PipelineError};
    use mqo_spec::{Filter, Mqo, MeasureRef, TimeIntel};
    use serde_json::json;

    fn make_catalog_with_calc(unique_name: &str, label: &str) -> serde_json::Value {
        json!({
            "columns": [
                {
                    "kind": "measure",
                    "unique_name": unique_name,
                    "label": label,
                    "is_calc": true
                }
            ]
        })
    }

    fn make_catalog_additive(unique_name: &str) -> serde_json::Value {
        json!({
            "columns": [
                {
                    "kind": "measure",
                    "unique_name": unique_name,
                    "label": "Revenue",
                    "is_calc": false
                }
            ]
        })
    }

    fn mqo_with_measure(unique_name: &str) -> Mqo {
        Mqo {
            model: "tpcds".to_string(),
            measures: vec![MeasureRef { unique_name: unique_name.to_string() }],
            dimensions: vec![],
            filters: vec![],
            time_intelligence: vec![],
            order: None,
            limit: None,
            non_empty: false,
            projection: false,
        }
    }

    /// AC-2: querying a calc measure without context → ParamRejected with
    /// missing_calc_context reason.
    #[test]
    fn calc_measure_no_context_is_rejected() {
        let catalog = make_catalog_with_calc(
            "tpcds.web_sales_increase",
            "Web Sales Increase",
        );
        let mqo = mqo_with_measure("tpcds.web_sales_increase");
        let result = check_calc_context(&mqo, &catalog);
        assert!(result.is_some(), "expected ParamRejected for calc measure without context");
        if let Some(PipelineError::ParamRejected { count, report }) = result {
            assert_eq!(count, 1);
            let rejections = report["param_rejections"].as_array().unwrap();
            assert_eq!(rejections.len(), 1);
            assert_eq!(rejections[0]["reason"], "missing_calc_context");
            assert_eq!(rejections[0]["field"], "tpcds.web_sales_increase");
        }
    }

    /// AC-2 via label alias: referencing by display label also triggers the decline.
    #[test]
    fn calc_measure_by_label_no_context_is_rejected() {
        let catalog = make_catalog_with_calc(
            "tpcds.web_sales_increase",
            "Web Sales Increase",
        );
        // Reference by label, not unique_name
        let mqo = mqo_with_measure("Web Sales Increase");
        let result = check_calc_context(&mqo, &catalog);
        assert!(result.is_some(), "label-based reference should also be caught");
        if let Some(PipelineError::ParamRejected { count, .. }) = result {
            assert_eq!(count, 1);
        }
    }

    /// AC-3 (pass-through): with YoY time_intelligence present, the check passes.
    #[test]
    fn calc_measure_with_yoy_passes() {
        let catalog = make_catalog_with_calc(
            "tpcds.web_sales_increase",
            "Web Sales Increase",
        );
        let mut mqo = mqo_with_measure("tpcds.web_sales_increase");
        mqo.time_intelligence = vec![TimeIntel::YoY];
        let result = check_calc_context(&mqo, &catalog);
        assert!(result.is_none(), "YoY context should allow the calc measure through");
    }

    /// AC-3 (pass-through): with PriorPeriod time_intelligence, passes.
    #[test]
    fn calc_measure_with_prior_period_passes() {
        let catalog = make_catalog_with_calc(
            "tpcds.web_sales_increase",
            "Web Sales Increase",
        );
        let mut mqo = mqo_with_measure("tpcds.web_sales_increase");
        mqo.time_intelligence = vec![TimeIntel::PriorPeriod];
        let result = check_calc_context(&mqo, &catalog);
        assert!(result.is_none(), "PriorPeriod context should allow the calc measure through");
    }

    /// AC-3 (pass-through): with CalcGroupMember filter, passes.
    #[test]
    fn calc_measure_with_calc_group_member_passes() {
        let catalog = make_catalog_with_calc(
            "tpcds.web_sales_increase",
            "Web Sales Increase",
        );
        let mut mqo = mqo_with_measure("tpcds.web_sales_increase");
        mqo.filters = vec![Filter::CalcGroupMember {
            calc_group: "Time Calculations".to_string(),
            member: "Prior Period".to_string(),
        }];
        let result = check_calc_context(&mqo, &catalog);
        assert!(result.is_none(), "CalcGroupMember filter should provide context");
    }

    /// AC-5: plain additive measures are unaffected (no false positive).
    #[test]
    fn plain_additive_measure_passes_through() {
        let catalog = make_catalog_additive("tpcds.web_sales");
        let mqo = mqo_with_measure("tpcds.web_sales");
        let result = check_calc_context(&mqo, &catalog);
        assert!(result.is_none(), "plain additive measure must not be rejected");
    }

    /// Conservative: measure not in catalog at all → passes through (no false-reject).
    #[test]
    fn unknown_measure_not_in_catalog_passes_through() {
        let catalog = make_catalog_with_calc(
            "tpcds.web_sales_increase",
            "Web Sales Increase",
        );
        // Different measure not in catalog
        let mqo = mqo_with_measure("tpcds.web_sales");
        let result = check_calc_context(&mqo, &catalog);
        assert!(result.is_none(), "measure not in catalog is not rejected");
    }
}
