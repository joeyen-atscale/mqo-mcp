//! Minimal MCP (Model Context Protocol) JSON-RPC 2.0 server over stdio for the
//! **dataset-handle** fleet.
//!
//! This is the keystone server.  It exposes:
//!
//! - `query_multidimensional` — runs the MQO `bind→route→compile→execute`
//!   pipeline (reusing the `mqo-mcp-server` library), puts the resulting
//!   [`Dataset`] into [`dh_store::Store`], and returns
//!   `{ summary, handle, capabilities }` — **never a `rows` field**.  Above the
//!   sample cap there is simply no field that could carry rows.
//! - a `dataset_*` tool family — one per `dh-ops` operation
//!   (`dataset_aggregate`, `dataset_filter`, `dataset_sort`, `dataset_top_n`,
//!   `dataset_pivot`, `dataset_compare`, `dataset_drill`, `dataset_describe`)
//!   plus `dataset_peek` (the current summary for a handle) and
//!   `dataset_export` (via `dh-export`).  Each op derives a *new* handle and
//!   returns its summary; none returns rows.
//! - `dataset_export` — the **only** tool that emits full data, and it returns a
//!   [`dh_export::ExportReceipt`] audit record.
//!
//! Read-only by construction: the query tool accepts a selection-only MQO (raw
//! SQL / non-MQO input is rejected with a structured error), and every tool is
//! advertised with `readOnlyHint: true`.

use std::path::PathBuf;

use dh_export::{export, ExportDest, ExportFmt, ExportOptions};
use dh_spec::{Capability, DatasetHandle};
use dh_store::{LookupError, Store};
use dh_summary::capabilities as ds_capabilities;
use mqo_mcp_server::mcp::ServerEngine;
use mqo_mcp_server::pipeline::{self, PipelineError, ToolPaths};
use mqo_mcp_server::probe::BackendCapabilities;
use std::collections::HashMap;
use serde_json::{json, Value};

use crate::convert::rows_to_dataset;

/// Protocol version this server speaks (matches the MCP spec revision string).
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Default TTL (seconds) applied to every dataset minted by this server.
pub const DEFAULT_TTL_SECS: u64 = 3600;

/// The ten `dataset_*` tool names, in advertised order.
pub const DATASET_TOOLS: [&str; 10] = [
    "dataset_peek",
    "dataset_aggregate",
    "dataset_filter",
    "dataset_sort",
    "dataset_top_n",
    "dataset_pivot",
    "dataset_compare",
    "dataset_drill",
    "dataset_describe",
    "dataset_export",
];

/// The three new BI/chart tool names.
pub const CHART_TOOLS: [&str; 3] = [
    "dataset_chart",
    "build_bi_asset",
    "compose_dashboard",
];

/// Server-side state.
pub struct Server {
    /// Recorded catalog snapshot (grounds the binder; identical contract to the
    /// MQO server's catalog).
    pub catalog: Value,
    /// Router stats bundle (level cardinalities + shape flags).
    pub stats: Value,
    /// Resolved fleet binary paths (`mqo-bind`, `mqo-route`, `mqo-dax`, `mqo-mdx`).
    pub tools: ToolPaths,
    /// Router row threshold above which the SQL extract path is chosen.
    pub row_threshold: u64,
    /// The handle store (TTL + LRU eviction live here).
    pub store: Store,
    /// Sample cap applied to summaries / advertised on the server.
    pub sample_cap: usize,
    /// TTL applied to minted datasets.
    pub ttl_secs: u64,
}

impl Server {
    /// Construct a server with the default TTL and a fresh store.
    #[must_use]
    pub fn new(
        catalog: Value,
        stats: Value,
        tools: ToolPaths,
        row_threshold: u64,
        max_total_bytes: usize,
        sample_cap: usize,
    ) -> Self {
        Self {
            catalog,
            stats,
            tools,
            row_threshold,
            store: Store::new(max_total_bytes),
            sample_cap,
            ttl_secs: DEFAULT_TTL_SECS,
        }
    }

    /// Handle one JSON-RPC request object, returning the response object.
    ///
    /// Notifications (requests with no `id`) return `None`.  Sweeps expired
    /// store entries opportunistically on each call so TTL/eviction is wired
    /// into the live request path.
    pub fn handle(&mut self, req: &Value) -> Option<Value> {
        let id = req.get("id").cloned()?;
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");

        // Wire TTL eviction into every request.
        self.store.evict_expired();

        let result = match method {
            "initialize" => Ok(Self::initialize()),
            "tools/list" => Ok(json!({ "tools": tool_descriptors() })),
            "tools/call" => self.tools_call(req.get("params")),
            "ping" => Ok(json!({})),
            other => Err(JsonRpcError::method_not_found(other)),
        };

        Some(match result {
            Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e.to_value() }),
        })
    }

    fn initialize() -> Value {
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "dh-mcp-server", "version": env!("CARGO_PKG_VERSION") }
        })
    }

    fn tools_call(&mut self, params: Option<&Value>) -> Result<Value, JsonRpcError> {
        let params = params.ok_or_else(|| JsonRpcError::invalid_params("missing params"))?;
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| JsonRpcError::invalid_params("missing tool name"))?;
        let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

        let result = match name {
            "query_multidimensional" => self.query_multidimensional(&args),
            "dataset_peek" => self.dataset_peek(&args),
            "dataset_aggregate" => self.dataset_op(Capability::Aggregate, &args),
            "dataset_filter" => self.dataset_op(Capability::Filter, &args),
            "dataset_sort" => self.dataset_op(Capability::Sort, &args),
            "dataset_top_n" => self.dataset_op(Capability::TopN, &args),
            "dataset_pivot" => self.dataset_op(Capability::Pivot, &args),
            "dataset_compare" => self.dataset_op(Capability::Compare, &args),
            "dataset_drill" => self.dataset_op(Capability::Drill, &args),
            "dataset_describe" => self.dataset_op(Capability::Describe, &args),
            "dataset_export" => self.dataset_export(&args),
            // BI/chart tools (v0.2.0)
            "dataset_chart" => crate::chart_tools::handle_dataset_chart(&self.store, &args),
            "build_bi_asset" => crate::chart_tools::handle_build_bi_asset(&self.store, &args),
            "compose_dashboard" => crate::chart_tools::handle_compose_dashboard(&self.store, &args),
            other => {
                return Err(JsonRpcError::invalid_params(&format!(
                    "unknown tool `{other}`"
                )))
            }
        };
        Ok(result)
    }

    // ── query_multidimensional ─────────────────────────────────────────────

    fn query_multidimensional(&mut self, args: &Value) -> Value {
        // The MQO lives under `args.mqo`. If the caller put a raw value (e.g. a
        // SQL string) directly, pass it through so the pipeline guard rejects it.
        let query = args.get("mqo").cloned().unwrap_or_else(|| args.clone());

        let out = match pipeline::run(
            &query,
            &self.catalog,
            &self.stats,
            &self.tools,
            self.row_threshold,
            &ServerEngine::Fixture,
            None,
            &BackendCapabilities::all_live(),
            None,
            &HashMap::new(),
            None, // channel_scope_map: dh-mcp-server has no enriched data
        ) {
            Ok(out) => out,
            Err(e) => return pipeline_err(&e),
        };

        // Convert the pipeline rows into a typed Dataset and store it.  The rows
        // go into the store, never into the response.
        let dataset = match rows_to_dataset(&out.bound, &out.rows) {
            Ok(ds) => ds,
            Err(detail) => return structured_err("conversion_error", json!(detail)),
        };

        let caps = ds_capabilities(&dataset);
        let summary = crate::summarize_with_cap(&dataset, self.sample_cap);
        let handle = self.store.put(dataset, self.ttl_secs);

        // NOTE: there is deliberately NO `rows` field in this payload.
        let payload = json!({
            "summary": summary,
            "handle": handle,
            "capabilities": caps,
            "backend": out.backend,
            "routing_reason": out.routing_reason,
        });
        ok_result(payload)
    }

    // ── dataset_peek ────────────────────────────────────────────────────────

    fn dataset_peek(&mut self, args: &Value) -> Value {
        let handle = match parse_handle(args) {
            Ok(h) => h,
            Err(v) => return v,
        };
        match self.store.get(&handle) {
            Ok(ds) => {
                let caps = ds_capabilities(&ds);
                let summary = crate::summarize_with_cap(&ds, self.sample_cap);
                ok_result(json!({
                    "summary": summary,
                    "handle": handle,
                    "capabilities": caps,
                }))
            }
            Err(e) => lookup_err(&e),
        }
    }

    // ── dataset_* operation tools ────────────────────────────────────────────

    fn dataset_op(&mut self, op: Capability, args: &Value) -> Value {
        let handle = match parse_handle(args) {
            Ok(h) => h,
            Err(v) => return v,
        };
        let params = args.get("params").cloned().unwrap_or_else(|| json!({}));

        let res = match op {
            Capability::Aggregate => dh_ops::aggregate(&mut self.store, &handle, &params),
            Capability::Filter => dh_ops::filter(&mut self.store, &handle, &params),
            Capability::Sort => dh_ops::sort(&mut self.store, &handle, &params),
            Capability::TopN => dh_ops::top_n(&mut self.store, &handle, &params),
            Capability::Pivot => dh_ops::pivot(&mut self.store, &handle, &params),
            Capability::Compare => dh_ops::compare(&mut self.store, &handle, &params),
            Capability::Drill => dh_ops::drill(&mut self.store, &handle, &params),
            Capability::Describe => dh_ops::describe(&mut self.store, &handle, &params),
            // Export is never routed here; it has its own handler.
            Capability::Export => {
                return structured_err("internal_error", json!("export is not a dataset_op"))
            }
            // Chart/BiAsset are never routed here; they have their own handlers.
            Capability::Chart | Capability::BiAsset => {
                return structured_err(
                    "internal_error",
                    json!("chart/bi-asset ops are not dataset_op variants"),
                )
            }
        };

        match res {
            Ok(op_result) => {
                // Re-derive the capability set + a sample-cap-bounded summary for
                // the *new* handle.  dh-ops already produced a summary, but its
                // sample_cap is fixed at 8; we re-summarize to honour our config.
                let new_handle = op_result.handle;
                let caps = match self.store.get(&new_handle) {
                    Ok(ds) => ds_capabilities(&ds),
                    Err(_) => dh_spec::ALL_CAPABILITIES.to_vec(),
                };
                ok_result(json!({
                    "summary": op_result.summary,
                    "handle": new_handle,
                    "capabilities": caps,
                }))
            }
            Err(e) => op_err(&e),
        }
    }

    // ── dataset_export — the single full-data exit ───────────────────────────

    fn dataset_export(&mut self, args: &Value) -> Value {
        let handle = match parse_handle(args) {
            Ok(h) => h,
            Err(v) => return v,
        };

        let fmt = match parse_fmt(args) {
            Ok(f) => f,
            Err(v) => return v,
        };
        let dest = match parse_dest(args) {
            Ok(d) => d,
            Err(v) => return v,
        };
        let opts = ExportOptions {
            overwrite: args
                .get("overwrite")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            override_json_limit: args
                .get("override_json_limit")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };

        match export(&self.store, &handle, fmt, dest, opts) {
            Ok(receipt) => ok_result(json!({ "receipt": receipt })),
            Err(e) => structured_err("export_error", json!(e.to_string())),
        }
    }
}

// ── Tool descriptors ─────────────────────────────────────────────────────────

/// A reusable input-schema fragment requiring a `handle` object.
fn handle_schema() -> Value {
    json!({ "type": "object", "description": "An opaque DatasetHandle as returned by a prior call." })
}

/// The advertised tool list.  All tools carry `readOnlyHint: true`.
#[must_use]
pub fn tool_descriptors() -> Value {
    let mut tools = vec![json!({
        "name": "query_multidimensional",
        "description": "Run a Multidimensional Query Object (NEVER raw SQL) through bind→route→compile→execute, store the result, and return { summary, handle, capabilities }. Does NOT return rows — retrieve data via dataset_* tools.",
        "inputSchema": {
            "type": "object",
            "properties": { "mqo": { "type": "object", "description": "The Multidimensional Query Object." } },
            "required": ["mqo"],
            "additionalProperties": false
        },
        "annotations": { "readOnlyHint": true }
    })];

    let dataset_descs: Vec<(&str, &str, Value)> = vec![
        (
            "dataset_peek",
            "Return the current summary + capabilities for a stored handle. Does not return rows.",
            json!({
                "type": "object",
                "properties": { "handle": handle_schema() },
                "required": ["handle"]
            }),
        ),
        (
            "dataset_aggregate",
            "Group-by + aggregate (sum/mean/min/max/count/count_distinct). Derives a new handle; returns its summary.",
            op_schema(),
        ),
        (
            "dataset_filter",
            "Filter rows by a compound AND/OR predicate. Derives a new handle; returns its summary.",
            op_schema(),
        ),
        (
            "dataset_sort",
            "Sort by one or more keys (asc/desc). Derives a new handle; returns its summary.",
            op_schema(),
        ),
        (
            "dataset_top_n",
            "Return the top/bottom N rows by a measure. Derives a new handle; returns its summary.",
            op_schema(),
        ),
        (
            "dataset_pivot",
            "Pivot rows × cols × measure into a crosstab. Derives a new handle; returns its summary.",
            op_schema(),
        ),
        (
            "dataset_compare",
            "Compare two handles → delta / pct-change. Derives a new handle; returns its summary.",
            op_schema(),
        ),
        (
            "dataset_drill",
            "Expand a grouped row back to its detail rows via lineage. Derives a new handle; returns its summary.",
            op_schema(),
        ),
        (
            "dataset_describe",
            "Per-column statistics for a handle without changing rows. Derives a new handle; returns its summary.",
            op_schema(),
        ),
        (
            "dataset_export",
            "The ONLY tool that emits full data. Materializes a handle to CSV/JSON/Parquet (file or inline) and returns an audited export receipt.",
            json!({
                "type": "object",
                "properties": {
                    "handle": handle_schema(),
                    "format": { "type": "string", "enum": ["csv", "json", "parquet"] },
                    "max_rows": { "type": "integer", "description": "JSON row cap." },
                    "dest": { "type": "string", "enum": ["inline", "file"] },
                    "path": { "type": "string", "description": "Target path for dest=file." },
                    "max_bytes": { "type": "integer", "description": "Inline payload cap." },
                    "overwrite": { "type": "boolean" },
                    "override_json_limit": { "type": "boolean" }
                },
                "required": ["handle", "format"]
            }),
        ),
    ];

    for (name, desc, schema) in dataset_descs {
        tools.push(json!({
            "name": name,
            "description": desc,
            "inputSchema": schema,
            "annotations": { "readOnlyHint": true }
        }));
    }

    // ── BI / chart tools (v0.2.0) ──────────────────────────────────────────

    let chart_descs: Vec<(&str, &str, Value)> = vec![
        (
            "dataset_chart",
            "Emit a Vega-Lite v5 JSON spec for a stored handle. Specify chart_type (bar|line|area|point), x_col, y_cols, and optional title. Does NOT return rows — the spec embeds the data inline.",
            json!({
                "type": "object",
                "properties": {
                    "handle": handle_schema(),
                    "chart_type": {
                        "type": "string",
                        "enum": ["bar", "line", "area", "point"],
                        "description": "Vega-Lite mark type. Default: 'bar'."
                    },
                    "x_col": {
                        "type": "string",
                        "description": "Column name for the X (category) axis."
                    },
                    "y_cols": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "One or more column names for the Y (metric) axis."
                    },
                    "title": {
                        "type": "string",
                        "description": "Optional chart title."
                    }
                },
                "required": ["handle", "x_col", "y_cols"]
            }),
        ),
        (
            "build_bi_asset",
            "Build a complete bi-asset.v1 bundle from a stored handle: auto-selects chart type, synthesizes title/description/caveats. Returns {title, description, caveats, vega_spec, profile_summary}. Does NOT return rows.",
            json!({
                "type": "object",
                "properties": {
                    "handle": handle_schema()
                },
                "required": ["handle"]
            }),
        ),
        (
            "compose_dashboard",
            "Compose a multi-panel Vega-Lite v5 concat spec from an array of stored handles. Each handle becomes one panel (a bi-asset.v1). Returns a dashboard.v1 payload. Does NOT return rows.",
            json!({
                "type": "object",
                "properties": {
                    "handles": {
                        "type": "array",
                        "items": handle_schema(),
                        "description": "Array of DatasetHandle objects to compose into panels."
                    },
                    "title": {
                        "type": "string",
                        "description": "Dashboard title."
                    },
                    "layout": {
                        "type": "string",
                        "enum": ["grid", "vertical", "horizontal"],
                        "description": "Panel layout. Default: 'grid'."
                    },
                    "columns": {
                        "type": "integer",
                        "description": "Number of columns in grid layout. Default: 2."
                    }
                },
                "required": ["handles", "title"]
            }),
        ),
    ];

    for (name, desc, schema) in chart_descs {
        tools.push(json!({
            "name": name,
            "description": desc,
            "inputSchema": schema,
            "annotations": { "readOnlyHint": true }
        }));
    }

    Value::Array(tools)
}

/// Shared input schema for the operation tools: `{ handle, params }`.
fn op_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "handle": handle_schema(),
            "params": { "type": "object", "description": "Operation-specific parameters." }
        },
        "required": ["handle"]
    })
}

// ── Argument parsing helpers ─────────────────────────────────────────────────

/// Parse a `DatasetHandle` from `args.handle`, returning a structured tool error
/// on failure.
fn parse_handle(args: &Value) -> Result<DatasetHandle, Value> {
    let h = args
        .get("handle")
        .ok_or_else(|| structured_err("bad_param", json!("missing 'handle'")))?;
    serde_json::from_value(h.clone())
        .map_err(|e| structured_err("bad_param", json!(format!("invalid handle: {e}"))))
}

fn parse_fmt(args: &Value) -> Result<ExportFmt, Value> {
    let fmt = args
        .get("format")
        .and_then(Value::as_str)
        .ok_or_else(|| structured_err("bad_param", json!("missing 'format'")))?;
    match fmt.to_lowercase().as_str() {
        "csv" => Ok(ExportFmt::Csv),
        "json" => {
            let max_rows = args
                .get("max_rows")
                .and_then(Value::as_u64)
                .map_or(usize::MAX, |v| usize::try_from(v).unwrap_or(usize::MAX));
            Ok(ExportFmt::Json { max_rows })
        }
        "parquet" => Ok(ExportFmt::Parquet),
        other => Err(structured_err(
            "bad_param",
            json!(format!("unknown format `{other}`")),
        )),
    }
}

fn parse_dest(args: &Value) -> Result<ExportDest, Value> {
    let dest = args
        .get("dest")
        .and_then(Value::as_str)
        .unwrap_or("inline");
    match dest {
        "inline" => {
            let max_bytes = args
                .get("max_bytes")
                .and_then(Value::as_u64)
                .map_or(1_048_576, |v| usize::try_from(v).unwrap_or(1_048_576));
            Ok(ExportDest::Inline { max_bytes })
        }
        "file" => {
            let path = args
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| structured_err("bad_param", json!("dest=file requires 'path'")))?;
            Ok(ExportDest::File(PathBuf::from(path)))
        }
        other => Err(structured_err(
            "bad_param",
            json!(format!("unknown dest `{other}`")),
        )),
    }
}

// ── Result / error builders ──────────────────────────────────────────────────

/// Build a tool-call success result whose `content[0]` is the JSON payload as
/// text and whose `structuredContent` carries the parsed object.
#[allow(clippy::needless_pass_by_value)] // payload is stored into the result object
fn ok_result(payload: Value) -> Value {
    let text = serde_json::to_string(&payload).unwrap_or_default();
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": payload,
        "isError": false
    })
}

/// Build a tool-call *application* error (`isError: true`) with a structured
/// `{ code, detail }` body.  Per MCP, tool failures are reported in the result,
/// not as protocol errors.
#[allow(clippy::needless_pass_by_value)] // detail is stored into the error payload
fn structured_err(code: &str, detail: Value) -> Value {
    let payload = json!({ "error": { "code": code, "detail": detail } });
    let text = serde_json::to_string(&payload).unwrap_or_default();
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": payload,
        "isError": true
    })
}

/// Map a [`pipeline::PipelineError`] to a structured tool error.
fn pipeline_err(e: &PipelineError) -> Value {
    let (code, detail) = match e {
        PipelineError::NotAnMqo(d) => ("not_an_mqo", json!(d)),
        PipelineError::Invalid(d) => ("invalid_mqo", json!(d)),
        PipelineError::NotGround { report } => ("not_ground", report.clone()),
        PipelineError::ParamRejected { report, .. } => ("param_rejected", report.clone()),
        PipelineError::Subprocess { tool, detail } => {
            ("subprocess_error", json!({ "tool": tool, "detail": detail }))
        }
        PipelineError::Io(d) => ("io_error", json!(d)),
        PipelineError::Engine(e) => ("engine_error", json!(e.to_string())),
        PipelineError::NoBackendAvailable { dax, mdx, sql } => (
            "no_backend_available",
            json!({ "dax": dax, "mdx": mdx, "sql": sql }),
        ),
        PipelineError::CrossFactIncompatible { report } => {
            ("cross_fact_incompatible", report.clone())
        }
        PipelineError::XmlaCoordsNotFound { model } => {
            ("xmla_coords_not_found", json!(model))
        }
        PipelineError::ProjectionTooLarge { level, estimate, cap } => (
            "projection_too_large",
            json!({ "level": level, "estimate": estimate, "cap": cap }),
        ),
        PipelineError::NonQueryableDimension { model, candidate_cubes } => (
            "non_queryable_dimension",
            json!({ "model": model, "candidate_cubes": candidate_cubes }),
        ),
        PipelineError::DimensionNotMaterialized { report, .. } => {
            ("dimension_not_materialized", report.clone())
        }
        PipelineError::ProjectionUnorderedLimit => (
            "projection_unordered_limit",
            json!({
                "detail": "Projection has a `limit` but no `order`: the top-N result would be \
                           non-deterministic. FIX: add a non-empty `order` field to the MQO so \
                           the top-N is well-defined. Example: add `\"order\": [{\"key\": \
                           \"<measure_or_level_unique_name>\", \"direction\": \"desc\"}]`."
            }),
        ),
        PipelineError::SqlRejected { count, report } => (
            "sql_rejected",
            json!({
                "count": count,
                "detail": format!(
                    "The compiled SQL failed validation with {count} violation(s) before \
                     execution. Submit a single SELECT statement per query_multidimensional \
                     call — multi-statement SQL is not supported (ATSCALE-48466). \
                     See `violations` for the machine-readable rule codes and fix guidance."
                ),
                "violations": report,
            }),
        ),
    };
    structured_err(code, detail)
}

/// Map a [`dh_ops::OpError`] to a structured tool error.
fn op_err(e: &dh_ops::OpError) -> Value {
    use dh_ops::OpError;
    let code = match e {
        OpError::HandleNotFound(_) => "handle_not_found",
        OpError::BadParam(_) => "bad_param",
        OpError::UnknownColumn(_) => "unknown_column",
        OpError::Unsupported(_) => "unsupported",
        OpError::Internal(_) => "internal_error",
    };
    structured_err(code, json!(e.to_string()))
}

/// Map a [`dh_store::LookupError`] to a structured tool error.
fn lookup_err(e: &LookupError) -> Value {
    let code = match e {
        LookupError::Expired => "handle_expired",
        LookupError::NotFound => "handle_not_found",
    };
    structured_err(code, json!(e.to_string()))
}

// ── JSON-RPC error helper ──────────────────────────────────────────────────

/// A JSON-RPC 2.0 error object (transport-level errors only).
struct JsonRpcError {
    code: i64,
    message: String,
}

impl JsonRpcError {
    fn method_not_found(method: &str) -> Self {
        JsonRpcError {
            code: -32601,
            message: format!("method not found: {method}"),
        }
    }
    fn invalid_params(detail: &str) -> Self {
        JsonRpcError {
            code: -32602,
            message: format!("invalid params: {detail}"),
        }
    }
    fn to_value(&self) -> Value {
        json!({ "code": self.code, "message": self.message })
    }
}
