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
        // Model/path: query construction or binding failures.
        PipelineError::NotGround { .. }
        | PipelineError::CrossFactIncompatible { .. }
        | PipelineError::ParamRejected { .. }
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
        | EngineError::RowCapTripped { .. } => INFRASTRUCTURE,
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
    if let Some(rejection) = param_validate(&mqo, catalog) {
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
    let bound_value = bind_step(tools, &mqo_path, &catalog_path, enriched_path.as_deref())?;
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
    let EngineResult { rows, .. } = match server_engine {
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

    Ok(PipelineOutput {
        backend,
        estimated_rows,
        routing_reason,
        compiled_query,
        rows,
        bound: bound_value,
        filters_applied,
        filters_dropped,
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
fn param_validate(mqo: &mqo_spec::Mqo, catalog: &Value) -> Option<PipelineError> {
    use mqo_param_validator::{
        validate, BoundMqoInput, CatalogHierarchy, CatalogMeasure, CatalogSnapshot,
        MqoDimensionRef, MqoFilterRef, MqoMeasureRef,
    };
    use std::collections::BTreeMap;

    let cols = catalog.get("columns").and_then(Value::as_array)?;

    let mut measures: Vec<CatalogMeasure> = Vec::new();
    // hierarchy -> set of level labels (preserve first-seen order via Vec + dedup)
    let mut hier_levels: BTreeMap<String, Vec<String>> = BTreeMap::new();

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
                // Primary entry under unique_name.
                measures.push(CatalogMeasure {
                    unique_name: un.to_string(),
                    subject_area: None,
                    label: label.clone(),
                    is_calc,
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
                mqo_spec::Filter::Member { hierarchy, .. } => Some(MqoFilterRef {
                    unique_name: hierarchy.clone(),
                    level: None,
                }),
                // Range/CalcGroupMember don't carry a resolvable hierarchy key
                // the validator can ground; skip (conservative).
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
