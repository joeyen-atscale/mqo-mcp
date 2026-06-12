//! Chart-tooling helpers: bridges `mqo-result-profiler` → `mqo-chart-recommender`
//! → `mqo-vega-emitter` for the `recommend_chart`, `build_vega_spec`,
//! `build_bi_asset`, and `compose_dashboard` MCP tools.
//!
//! All functions in this module are deterministic and side-effect-free (no I/O,
//! no network, no disk). All MCP tools in this module carry `readOnlyHint: true`.

use mqo_chart_recommender::ChartRecommendation;
use mqo_result_profiler::{DataType, Role};
use serde_json::{json, Value};

// ── Internal conversion helpers ───────────────────────────────────────────────

/// Convert a [`mqo_result_profiler::ResultProfile`] to the `result-profile.v1`
/// JSON shape expected by `mqo-chart-recommender`'s `recommend()` function.
///
/// The recommender reads:
/// ```json
/// {
///   "schema": "result-profile.v1",
///   "columns": [
///     {"name": "...", "role": "measure"|"dimension",
///      "is_temporal": bool, "cardinality": u64|null}
///   ]
/// }
/// ```
#[must_use]
pub fn result_profile_to_recommender_json(profile: &mqo_result_profiler::ResultProfile) -> Value {
    let columns: Vec<Value> = profile
        .columns
        .iter()
        .map(|col| {
            let role = match col.role {
                Role::Measure => "measure",
                Role::Dimension => "dimension",
            };
            let is_temporal = col.data_type == DataType::Temporal;
            // cardinality is usize; the recommender accepts u64|null.
            // For measures cardinality is 0 (or the distinct-value count) — pass it.
            json!({
                "name": col.name,
                "role": role,
                "is_temporal": is_temporal,
                "cardinality": col.cardinality
            })
        })
        .collect();
    json!({
        "schema": "result-profile.v1",
        "columns": columns
    })
}

/// Convert a [`ChartRecommendation`] to the JSON shape expected by
/// `mqo-vega-emitter`'s `emit()` function.
///
/// The recommender serialises `Mark` with `serde(rename_all = "snake_case")`
/// (`Mark::Line` → `"line"`, `Mark::BigNumber` → `"big_number"`), but the
/// emitter's `map_mark()` pattern-matches `PascalCase` strings (`"Line"`,
/// `"BigNumber"`). We re-serialise the mark into `PascalCase` form here.
///
/// Encoding `Channel.data_type` strings (`"quantitative"` etc.) are already
/// compatible between the two crates.
pub fn recommendation_to_emitter_json(rec: &ChartRecommendation) -> Value {
    // Serialise the recommendation to Value first; this gives us the
    // snake_case mark string and the encoding in the right shape.
    let mut val = serde_json::to_value(rec).unwrap_or_else(|_| json!({}));

    // Normalise mark: snake_case → PascalCase for the emitter.
    if let Some(mark_str) = val.get("mark").and_then(Value::as_str) {
        let pascal = snake_to_pascal(mark_str);
        val["mark"] = Value::String(pascal);
    }
    val
}

/// Convert a `recommendation` JSON supplied by the caller (which may already
/// be `PascalCase` or `snake_case` from any source) into the `PascalCase` form the
/// emitter requires.
///
/// Accepts both `"line"` / `"big_number"` (recommender serialised) and `"Line"`
/// / `"BigNumber"` (emitter-native or hand-written). No-ops if already `PascalCase`.
pub fn normalize_recommendation_for_emitter(rec: &Value) -> Value {
    let mut out = rec.clone();
    if let Some(mark_str) = out.get("mark").and_then(Value::as_str) {
        // Only normalise if it looks like it needs it (starts lowercase).
        if mark_str.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
            out["mark"] = Value::String(snake_to_pascal(mark_str));
        }
    }
    out
}

/// Convert a `snake_case` string to `PascalCase`.
///
/// `"line"` → `"Line"`, `"big_number"` → `"BigNumber"`.
fn snake_to_pascal(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    first.to_uppercase().collect::<String>() + chars.as_str()
                }
            }
        })
        .collect()
}

// ── Public tool handlers (called from mcp.rs) ─────────────────────────────────

/// Handle the `recommend_chart` MCP tool.
///
/// Accepts either:
/// - `{ "response": <query_multidimensional payload> }` — the full `{rows, bound}` object, or
/// - `{ "rows": [...], "bound": {...} }` directly.
///
/// Returns a `chart-recommendation.v1` JSON in `structuredContent`.
#[must_use]
pub fn handle_recommend_chart(args: &Value, catalog: &Value) -> Value {
    // Resolve the payload containing rows + bound.
    let payload = if let Some(resp) = args.get("response") {
        resp.clone()
    } else if args.get("rows").is_some() && args.get("bound").is_some() {
        args.clone()
    } else {
        return chart_err(
            "invalid_input",
            "recommend_chart requires either a `response` key (query_multidimensional payload) \
             or both `rows` and `bound` keys",
        );
    };

    // Step 1: profile.
    let profile = match mqo_result_profiler::profile(&payload, catalog) {
        Ok(p) => p,
        Err(e) => {
            return chart_err("profile_error", &e.to_string());
        }
    };

    // Step 2: convert to recommender JSON and recommend.
    let recommender_json = result_profile_to_recommender_json(&profile);
    let recommendation = match mqo_chart_recommender::recommend(&recommender_json) {
        Ok(r) => r,
        Err(e) => {
            return chart_err("recommend_error", &e.to_string());
        }
    };

    // Step 3: serialise to Value (uses snake_case from the recommender's serde impl).
    let rec_val = serde_json::to_value(&recommendation).unwrap_or_else(|e| {
        json!({ "error": e.to_string() })
    });

    chart_ok(&rec_val)
}

/// Handle the `build_vega_spec` MCP tool.
///
/// Accepts either:
/// - `{ "response": <query_multidimensional payload> }` — full pipeline, or
/// - `{ "recommendation": <chart-recommendation.v1>, "rows": [...] }` — emit-only.
///
/// Returns a Vega-Lite v5 spec JSON in `structuredContent`.
pub fn handle_build_vega_spec(args: &Value, catalog: &Value) -> Value {
    let (recommendation_json, rows) = if let Some(resp) = args.get("response") {
        // Full pipeline: profile → recommend → emit.
        let profile = match mqo_result_profiler::profile(resp, catalog) {
            Ok(p) => p,
            Err(e) => return chart_err("profile_error", &e.to_string()),
        };

        let recommender_json = result_profile_to_recommender_json(&profile);
        let recommendation = match mqo_chart_recommender::recommend(&recommender_json) {
            Ok(r) => r,
            Err(e) => return chart_err("recommend_error", &e.to_string()),
        };

        let rows_val: Vec<Value> = resp
            .get("rows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let rec_json = recommendation_to_emitter_json(&recommendation);
        (rec_json, rows_val)
    } else if let (Some(rec), Some(rows_val)) = (args.get("recommendation"), args.get("rows")) {
        // Emit-only: use the supplied recommendation + rows.
        let rows: Vec<Value> = rows_val.as_array().cloned().unwrap_or_default();
        let rec_normalised = normalize_recommendation_for_emitter(rec);
        (rec_normalised, rows)
    } else {
        return chart_err(
            "invalid_input",
            "build_vega_spec requires either a `response` key (query_multidimensional payload) \
             or both `recommendation` and `rows` keys",
        );
    };

    // Step: emit the Vega-Lite spec.
    match mqo_vega_emitter::emit(&recommendation_json, &rows) {
        Ok(spec) => chart_ok(&spec),
        Err(e) => chart_err("emit_error", &e.to_string()),
    }
}

/// Maximum row count accepted by `build_bi_asset`. Mirrors the `INLINE_THRESHOLD`
/// discipline from `handle_ops.rs`.
pub const BUILD_BI_ASSET_MAX_ROWS: usize = 500;

/// Maximum panel count accepted by `compose_dashboard`.
pub const COMPOSE_DASHBOARD_MAX_PANELS: usize = 20;

/// Handle the `build_bi_asset` MCP tool.
///
/// Accepts either:
/// - `{ "response": <query_multidimensional payload> }` — the full `{rows, bound}` object, or
/// - `{ "rows": [...], "bound": {...} }` directly.
///
/// Returns a `bi-asset.v1` payload (`{asset, title, description, vega_spec,
/// profile_summary, caveats}`) in `structuredContent`.
///
/// Read-only by construction — no state mutation, deterministic, idempotent.
/// Returns an error envelope when the input exceeds [`BUILD_BI_ASSET_MAX_ROWS`]
/// rather than truncating.
#[must_use]
pub fn handle_build_bi_asset(args: &Value, catalog: &Value) -> Value {
    // Resolve the payload containing rows + bound.
    let payload = if let Some(resp) = args.get("response") {
        resp.clone()
    } else if args.get("rows").is_some() && args.get("bound").is_some() {
        args.clone()
    } else {
        return chart_err(
            "invalid_input",
            "build_bi_asset requires either a `response` key (query_multidimensional payload) \
             or both `rows` and `bound` keys",
        );
    };

    // Bounded-input guard: reject empty or over-sized row arrays.
    let row_count = payload
        .get("rows")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if row_count == 0 {
        return chart_err(
            "empty_rows",
            "build_bi_asset requires at least one row; the response contained no rows.",
        );
    }
    if row_count > BUILD_BI_ASSET_MAX_ROWS {
        return chart_err(
            "input_too_large",
            &format!(
                "build_bi_asset accepts at most {BUILD_BI_ASSET_MAX_ROWS} rows; \
                 got {row_count}. Reduce the query result before calling this tool."
            ),
        );
    }

    match mqo_bi_asset_bundle::build_asset(&payload, catalog) {
        Ok(asset) => {
            let asset_val = serde_json::to_value(&asset).unwrap_or_else(|e| {
                serde_json::json!({ "error": e.to_string() })
            });
            chart_ok(&asset_val)
        }
        Err(e) => chart_err("build_asset_error", &e.to_string()),
    }
}

/// Handle the `compose_dashboard` MCP tool.
///
/// Accepts:
/// - `bundles`: required array of `bi-asset.v1` JSON objects.
/// - `title`: required string — the dashboard title.
/// - `layout`: optional `"grid"` | `"vertical"` | `"horizontal"` (default: `"grid"`).
/// - `columns`: optional integer grid width (default: 2).
///
/// Returns a `dashboard.v1` payload (`{dashboard, title, layout, columns, panels[],
/// vega_concat_spec}`) in `structuredContent`.
///
/// Read-only by construction — no state mutation, deterministic, idempotent.
/// Returns an error envelope when panel count is zero or exceeds
/// [`COMPOSE_DASHBOARD_MAX_PANELS`].
#[must_use]
pub fn handle_compose_dashboard(args: &Value) -> Value {
    // Resolve `title`.
    let Some(title) = args.get("title").and_then(Value::as_str) else {
        return chart_err("invalid_input", "compose_dashboard requires a `title` string");
    };

    // Resolve `bundles` array.
    let Some(bundles_val) = args.get("bundles").and_then(Value::as_array) else {
        return chart_err(
            "invalid_input",
            "compose_dashboard requires a `bundles` array of bi-asset.v1 objects",
        );
    };

    // Zero-panel guard.
    if bundles_val.is_empty() {
        return chart_err(
            "no_panels",
            "compose_dashboard requires at least one panel in `bundles`",
        );
    }

    // Over-bound guard.
    if bundles_val.len() > COMPOSE_DASHBOARD_MAX_PANELS {
        return chart_err(
            "input_too_large",
            &format!(
                "compose_dashboard accepts at most {COMPOSE_DASHBOARD_MAX_PANELS} panels; \
                 got {}. Reduce the bundle count.",
                bundles_val.len()
            ),
        );
    }

    // Parse layout (optional, default "grid").
    let layout = match args.get("layout").and_then(Value::as_str).unwrap_or("grid") {
        "vertical" => mqo_dashboard_composer::Layout::Vertical,
        "horizontal" => mqo_dashboard_composer::Layout::Horizontal,
        _ => mqo_dashboard_composer::Layout::Grid,
    };

    // Parse columns (optional, default 2).
    let columns: u32 = args
        .get("columns")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(2)
        .max(1);

    // Deserialise each bundle.
    let mut bundles: Vec<mqo_dashboard_composer::BiAssetBundle> = Vec::new();
    for (i, b) in bundles_val.iter().enumerate() {
        match serde_json::from_value::<mqo_dashboard_composer::BiAssetBundle>(b.clone()) {
            Ok(bundle) => bundles.push(bundle),
            Err(e) => {
                return chart_err(
                    "invalid_bundle",
                    &format!("bundle[{i}] could not be parsed as bi-asset.v1: {e}"),
                );
            }
        }
    }

    let dashboard = mqo_dashboard_composer::build_dashboard(&bundles, title, layout, columns);
    let dashboard_val = serde_json::to_value(&dashboard).unwrap_or_else(|e| {
        serde_json::json!({ "error": e.to_string() })
    });
    chart_ok(&dashboard_val)
}

// ── Result envelope helpers ────────────────────────────────────────────────────

fn chart_ok(payload: &Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string(payload).unwrap_or_default() }],
        "structuredContent": payload,
        "isError": false
    })
}

fn chart_err(code: &str, detail: &str) -> Value {
    let payload = json!({ "error": { "code": code, "detail": detail } });
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string(&payload).unwrap_or_default() }],
        "structuredContent": payload,
        "isError": true
    })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_to_pascal_basic() {
        assert_eq!(snake_to_pascal("line"), "Line");
        assert_eq!(snake_to_pascal("big_number"), "BigNumber");
        assert_eq!(snake_to_pascal("bar"), "Bar");
        assert_eq!(snake_to_pascal("table"), "Table");
    }

    #[test]
    fn normalize_already_pascal_is_noop() {
        let rec = json!({"mark": "Line", "encoding": {}});
        let out = normalize_recommendation_for_emitter(&rec);
        assert_eq!(out["mark"], "Line");
    }

    #[test]
    fn normalize_snake_becomes_pascal() {
        let rec = json!({"mark": "line", "encoding": {}});
        let out = normalize_recommendation_for_emitter(&rec);
        assert_eq!(out["mark"], "Line");
    }

    // ── build_bi_asset tests ───────────────────────────────────────────────────

    fn sample_response_and_catalog() -> (Value, Value) {
        let response = json!({
            "rows": [
                {"revenue": 100.0, "year": "2021"},
                {"revenue": 200.0, "year": "2022"}
            ],
            "bound": { "measures": ["revenue"], "dimensions": ["year"] }
        });
        let catalog = json!({
            "columns": [
                {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
                {"unique_name": "year", "label": "Year", "kind": "dimension",
                 "hierarchy": "time.calendar"}
            ]
        });
        (response, catalog)
    }

    #[test]
    fn build_bi_asset_happy_path() {
        let (response, catalog) = sample_response_and_catalog();
        let args = json!({ "response": response });
        let result = handle_build_bi_asset(&args, &catalog);
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["asset"], "bi-asset.v1");
        assert!(!result["structuredContent"]["title"].as_str().unwrap_or("").is_empty());
        assert!(!result["structuredContent"]["description"].as_str().unwrap_or("").is_empty());
        assert!(result["structuredContent"]["vega_spec"].is_object());
        assert!(result["structuredContent"]["caveats"].is_array());
    }

    #[test]
    fn build_bi_asset_rows_bound_shape() {
        let (response, catalog) = sample_response_and_catalog();
        // Pass rows+bound directly (not nested under "response").
        let args = json!({
            "rows": response["rows"],
            "bound": response["bound"]
        });
        let result = handle_build_bi_asset(&args, &catalog);
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["asset"], "bi-asset.v1");
    }

    #[test]
    fn build_bi_asset_empty_rows_returns_error() {
        let catalog = json!({"columns": []});
        let args = json!({ "response": { "rows": [], "bound": { "measures": [], "dimensions": [] } } });
        let result = handle_build_bi_asset(&args, &catalog);
        assert_eq!(result["isError"], true);
        assert!(result["structuredContent"]["error"]["code"].as_str().is_some());
    }

    #[test]
    fn build_bi_asset_missing_keys_returns_invalid_input() {
        let catalog = json!({"columns": []});
        let args = json!({ "something_else": true });
        let result = handle_build_bi_asset(&args, &catalog);
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["error"]["code"], "invalid_input");
    }

    #[test]
    fn build_bi_asset_over_bound_returns_error() {
        let catalog = json!({"columns": [
            {"unique_name": "v", "label": "V", "kind": "measure"}
        ]});
        // Construct a row array that exceeds BUILD_BI_ASSET_MAX_ROWS.
        let rows: Vec<Value> = (0..=BUILD_BI_ASSET_MAX_ROWS)
            .map(|i| json!({"v": i}))
            .collect();
        let args = json!({
            "response": {
                "rows": rows,
                "bound": {"measures": ["v"], "dimensions": []}
            }
        });
        let result = handle_build_bi_asset(&args, &catalog);
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["error"]["code"], "input_too_large");
    }

    // ── compose_dashboard tests ───────────────────────────────────────────────

    fn make_bi_asset_bundle_val(title: &str) -> Value {
        json!({
            "asset": "bi-asset.v1",
            "title": title,
            "description": format!("Description for {title}"),
            "vega_spec": {"mark": "bar", "$schema": "https://vega.github.io/schema/vega-lite/v5.json"},
            "profile_summary": null,
            "caveats": []
        })
    }

    #[test]
    fn compose_dashboard_happy_path() {
        let args = json!({
            "bundles": [
                make_bi_asset_bundle_val("Panel A"),
                make_bi_asset_bundle_val("Panel B")
            ],
            "title": "My Dashboard",
            "layout": "grid",
            "columns": 2
        });
        let result = handle_compose_dashboard(&args);
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["dashboard"], "dashboard.v1");
        assert_eq!(result["structuredContent"]["title"], "My Dashboard");
        let panels = result["structuredContent"]["panels"].as_array().unwrap();
        assert_eq!(panels.len(), 2);
        assert!(result["structuredContent"]["vega_concat_spec"].is_object());
    }

    #[test]
    fn compose_dashboard_default_layout_is_grid() {
        let args = json!({
            "bundles": [make_bi_asset_bundle_val("Solo")],
            "title": "T"
        });
        let result = handle_compose_dashboard(&args);
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["layout"], "grid");
    }

    #[test]
    fn compose_dashboard_vertical_layout() {
        let args = json!({
            "bundles": [
                make_bi_asset_bundle_val("A"),
                make_bi_asset_bundle_val("B")
            ],
            "title": "T",
            "layout": "vertical"
        });
        let result = handle_compose_dashboard(&args);
        assert_eq!(result["isError"], false);
        assert_eq!(result["structuredContent"]["layout"], "vertical");
        assert!(result["structuredContent"]["vega_concat_spec"]["vconcat"].is_array());
    }

    #[test]
    fn compose_dashboard_zero_panels_returns_error() {
        let args = json!({ "bundles": [], "title": "Empty" });
        let result = handle_compose_dashboard(&args);
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["error"]["code"], "no_panels");
    }

    #[test]
    fn compose_dashboard_over_bound_returns_error() {
        let bundles: Vec<Value> = (0..=COMPOSE_DASHBOARD_MAX_PANELS)
            .map(|i| make_bi_asset_bundle_val(&format!("Panel {i}")))
            .collect();
        let args = json!({ "bundles": bundles, "title": "Too Many" });
        let result = handle_compose_dashboard(&args);
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["error"]["code"], "input_too_large");
    }

    #[test]
    fn compose_dashboard_missing_title_returns_error() {
        let args = json!({ "bundles": [make_bi_asset_bundle_val("A")] });
        let result = handle_compose_dashboard(&args);
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["error"]["code"], "invalid_input");
    }

    #[test]
    fn compose_dashboard_missing_bundles_returns_error() {
        let args = json!({ "title": "T" });
        let result = handle_compose_dashboard(&args);
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["error"]["code"], "invalid_input");
    }
}
