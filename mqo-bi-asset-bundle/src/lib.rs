//! `mqo-bi-asset-bundle` — one-shot MCP query response → titled, captioned BI asset.
//!
//! Takes a raw `query_multidimensional` response plus a catalog JSON, runs the
//! three-stage chain (profiler → recommender → emitter), and returns a complete
//! `bi-asset.v1` bundle: title, description, Vega-Lite v5 spec, profile summary,
//! and caveats.
//!
//! # Example
//!
//! ```rust
//! use mqo_bi_asset_bundle::build_asset;
//! use serde_json::json;
//!
//! let response = json!({
//!     "rows": [{"revenue": 100.0, "year": "2021"}],
//!     "bound": { "measures": ["revenue"], "dimensions": ["year"] }
//! });
//! let catalog = json!({
//!     "columns": [
//!         {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
//!         {"unique_name": "year", "label": "Year", "kind": "dimension",
//!          "hierarchy": "time.calendar"}
//!     ]
//! });
//! let asset = build_asset(&response, &catalog).expect("build_asset succeeds for valid inputs");
//! assert_eq!(asset.title, "Revenue by Year");
//! ```

#![forbid(unsafe_code)]

use mqo_chart_recommender::{Mark, recommend};
use mqo_result_profiler::{DataType, Role, ResultProfile, profile};
use mqo_vega_emitter::emit;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

// ─── public error type ────────────────────────────────────────────────────────

/// Errors that can occur while building a [`BiAsset`].
#[derive(Debug, Error)]
pub enum BundleError {
    /// The response payload was malformed (missing `rows` or `bound`).
    #[error("malformed response: {0}")]
    MalformedResponse(String),
    /// The chart recommender returned an error.
    #[error("recommender error: {0}")]
    RecommenderError(String),
    /// The Vega-Lite emitter returned an error.
    #[error("emitter error: {0}")]
    EmitterError(String),
}

// ─── public output types ──────────────────────────────────────────────────────

/// Condensed profile summary embedded in the bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileSummary {
    /// Total row count from the response.
    pub row_count: usize,
    /// Human-readable labels of measure columns.
    pub measures: Vec<String>,
    /// Human-readable labels of dimension columns.
    pub dimensions: Vec<String>,
}

/// The `bi-asset.v1` output bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiAsset {
    /// Schema version tag.
    pub asset: String,
    /// Human-readable chart title derived from catalog labels.
    pub title: String,
    /// One-sentence description derived from the chosen mark and fields.
    pub description: String,
    /// Full inline Vega-Lite v5 spec.
    pub vega_spec: Value,
    /// Condensed profile summary.
    pub profile_summary: ProfileSummary,
    /// Semantic caveats for the chart — empty when none apply.
    pub caveats: Vec<String>,
}

// ─── public entry point ───────────────────────────────────────────────────────

/// Build a `bi-asset.v1` bundle from a `query_multidimensional` response and
/// a catalog JSON.
///
/// # Errors
///
/// Returns [`BundleError`] if the response is malformed, or if the recommender
/// or emitter fails.
pub fn build_asset(response: &Value, catalog: &Value) -> Result<BiAsset, BundleError> {
    // Stage 1: profile
    let result_profile = profile(response, catalog).map_err(|e| {
        BundleError::MalformedResponse(e.to_string())
    })?;

    // Extract rows for the emitter
    let rows = extract_rows(response);

    // Stage 2: recommend — convert ResultProfile → profile JSON expected by recommender
    let profile_json = profile_to_recommender_json(&result_profile);
    let recommendation = recommend(&profile_json)
        .map_err(|e| BundleError::RecommenderError(e.to_string()))?;

    // Stage 3: emit vega spec.
    // mqo-vega-emitter's map_mark() expects PascalCase mark strings ("Line", "Bar", …),
    // but mqo-chart-recommender serialises Mark with snake_case ("line", "bar", …).
    // We build the emitter-compatible JSON by hand, patching the mark string.
    let rec_json = recommendation_to_emitter_json(&recommendation)?;
    let vega_spec = emit(&rec_json, &rows)
        .map_err(|e| BundleError::EmitterError(e.to_string()))?;

    // Synthesis
    let title = synthesize_title(&result_profile);
    let description = synthesize_description(&result_profile, &recommendation.mark);
    let caveats = synthesize_caveats(&result_profile, &recommendation.mark);
    let profile_summary = condense_profile(&result_profile);

    Ok(BiAsset {
        asset: "bi-asset.v1".to_owned(),
        title,
        description,
        vega_spec,
        profile_summary,
        caveats,
    })
}

// ─── internal helpers ─────────────────────────────────────────────────────────

/// Extract the `rows` array from the response (direct or MCP envelope).
fn extract_rows(response: &Value) -> Vec<Value> {
    // Direct shape
    if let Some(rows) = response.get("rows").and_then(|v| v.as_array()) {
        return rows.clone();
    }
    // MCP structuredContent envelope
    if let Some(sc) = response.get("structuredContent").and_then(|v| v.as_array()) {
        if let Some(first) = sc.first() {
            if let Some(inner) = first.get("json") {
                if let Some(rows) = inner.get("rows").and_then(|v| v.as_array()) {
                    return rows.clone();
                }
            }
        }
    }
    vec![]
}

/// Convert a [`ResultProfile`] into the `result-profile.v1`-ish JSON that
/// `mqo-chart-recommender`'s `recommend()` accepts.
fn profile_to_recommender_json(rp: &ResultProfile) -> Value {
    let columns: Vec<Value> = rp
        .columns
        .iter()
        .map(|col| {
            let role = match col.role {
                Role::Measure => "measure",
                Role::Dimension => "dimension",
            };
            let is_temporal = matches!(col.data_type, DataType::Temporal);
            // usize→u64: safe on every supported target (usize ≤ u64).
            let cardinality_u64 = u64::try_from(col.cardinality).unwrap_or(u64::MAX);
            serde_json::json!({
                "name": col.name,
                "role": role,
                "is_temporal": is_temporal,
                "cardinality": cardinality_u64,
            })
        })
        .collect();

    serde_json::json!({
        "schema": "result-profile.v1",
        "columns": columns,
    })
}

/// Map a [`Mark`] variant to the `PascalCase` string that `mqo-vega-emitter`'s
/// `map_mark()` expects ("Line", "Bar", "`BigNumber`", …).
///
/// `mqo-chart-recommender` serialises `Mark` with `#[serde(rename_all = "snake_case")]`
/// so a `to_value()` round-trip gives `"line"` / `"bar"`, but the emitter expects
/// the original enum variant names in `PascalCase`. This function bridges the gap
/// without coupling us to the emitter's private `map_mark` string table.
const fn mark_to_emitter_str(mark: &Mark) -> &'static str {
    match mark {
        Mark::Line => "Line",
        Mark::Bar => "Bar",
        Mark::Point => "Point",
        Mark::Area => "Area",
        Mark::Rect => "Rect",
        Mark::BigNumber => "BigNumber",
        Mark::Table => "Table",
    }
}

/// Build the JSON object that `mqo-vega-emitter::emit()` accepts from a
/// [`ChartRecommendation`], patching the mark string to `PascalCase`.
fn recommendation_to_emitter_json(
    rec: &mqo_chart_recommender::ChartRecommendation,
) -> Result<Value, BundleError> {
    // Serialise the full recommendation, then patch the `mark` field.
    let mut json = serde_json::to_value(rec)
        .map_err(|e| BundleError::EmitterError(e.to_string()))?;
    if let Some(obj) = json.as_object_mut() {
        obj.insert(
            "mark".to_owned(),
            Value::String(mark_to_emitter_str(&rec.mark).to_owned()),
        );
    }
    Ok(json)
}

/// Synthesize a human title from the catalog labels in the profile.
///
/// Rules (from PRD):
/// - one measure, one dimension → `"<Measure> by <Dimension>"`
/// - multiple measures, no dimension → `"<M1> and <M2>"`
/// - one measure, no dimension → `"<Measure> (total)"`
/// - one measure, multiple dimensions → `"<Measure> by <D1> and <D2>"`
fn synthesize_title(rp: &ResultProfile) -> String {
    let measures: Vec<&str> = rp
        .columns
        .iter()
        .filter(|c| c.role == Role::Measure)
        .map(|c| c.label.as_str())
        .collect();

    let dimensions: Vec<&str> = rp
        .columns
        .iter()
        .filter(|c| c.role == Role::Dimension)
        .map(|c| c.label.as_str())
        .collect();

    match (measures.len(), dimensions.len()) {
        (0, _) => "Data".to_owned(),
        (1, 0) => {
            let m = measures.first().copied().unwrap_or("");
            format!("{m} (total)")
        }
        (1, 1) => {
            let m = measures.first().copied().unwrap_or("");
            let d = dimensions.first().copied().unwrap_or("");
            format!("{m} by {d}")
        }
        (1, _) => {
            let m = measures.first().copied().unwrap_or("");
            let dims = join_labels(&dimensions);
            format!("{m} by {dims}")
        }
        (_, 0) => join_labels(&measures),
        (_, _) => {
            let ms = join_labels(&measures);
            let dims = join_labels(&dimensions);
            format!("{ms} by {dims}")
        }
    }
}

/// Join label slices as "A and B" or "A, B, and C".
fn join_labels(labels: &[&str]) -> String {
    match labels {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [a, b] => format!("{a} and {b}"),
        _ => {
            let (last, rest) = labels.split_last().unwrap_or((&"", &[]));
            format!("{}, and {last}", rest.join(", "))
        }
    }
}

/// Synthesize a one-sentence description from the recommended mark + field labels.
///
/// Rules (from PRD):
/// - aggregating mark (line/bar/area) over a dimension → `"Sum of <Measure> across <Dimension>."`
/// - two-measure scatter → `"<Measure A> vs <Measure B>."`
/// - KPI / `BigNumber` → `"Total <Measure>."`
/// - table fallback → `"<Measure> data table."`
fn synthesize_description(rp: &ResultProfile, mark: &Mark) -> String {
    let measures: Vec<&str> = rp
        .columns
        .iter()
        .filter(|c| c.role == Role::Measure)
        .map(|c| c.label.as_str())
        .collect();

    let dimensions: Vec<&str> = rp
        .columns
        .iter()
        .filter(|c| c.role == Role::Dimension)
        .map(|c| c.label.as_str())
        .collect();

    match mark {
        Mark::Line | Mark::Bar | Mark::Area => {
            if let (Some(&m), Some(&d)) = (measures.first(), dimensions.first()) {
                format!("Sum of {m} across {d}.")
            } else if let Some(&m) = measures.first() {
                format!("Sum of {m}.")
            } else {
                "Data chart.".to_owned()
            }
        }
        Mark::Point => {
            if let (Some(&m0), Some(&m1)) = (measures.first(), measures.get(1)) {
                format!("{m0} vs {m1}.")
            } else if let (Some(&m), Some(&d)) = (measures.first(), dimensions.first()) {
                format!("{m} by {d}.")
            } else {
                "Scatter plot.".to_owned()
            }
        }
        Mark::BigNumber => {
            if let Some(&m) = measures.first() {
                format!("Total {m}.")
            } else {
                "Single value.".to_owned()
            }
        }
        Mark::Rect => {
            if let (Some(&m), Some(&d1), Some(&d2)) =
                (measures.first(), dimensions.first(), dimensions.get(1))
            {
                format!("{m} by {d1} and {d2}.")
            } else {
                "Heatmap.".to_owned()
            }
        }
        Mark::Table => {
            if let Some(&m) = measures.first() {
                format!("{m} data table.")
            } else {
                "Data table.".to_owned()
            }
        }
    }
}

/// Synthesize semantic caveats from the profile + chosen mark.
///
/// Caveat rules (from PRD):
/// (a) `semi_additive` measure plotted summed over a temporal axis → aggregation risk.
/// (b) `is_calc` percentage measure in stacked/summed bar → summing-a-pct warning.
/// (c) nominal axis with cardinality > 25 → clutter caveat.
fn synthesize_caveats(rp: &ResultProfile, mark: &Mark) -> Vec<String> {
    const HIGH_CARDINALITY: usize = 25;
    let mut caveats = Vec::new();

    let has_temporal_dim = rp
        .columns
        .iter()
        .any(|c| c.role == Role::Dimension && matches!(c.data_type, DataType::Temporal));

    let is_aggregating = matches!(mark, Mark::Line | Mark::Bar | Mark::Area);

    // (a) semi-additive measure over temporal axis with aggregating mark
    if has_temporal_dim && is_aggregating {
        for col in rp.columns.iter().filter(|c| c.role == Role::Measure && c.semi_additive) {
            let mark_str = mark_display(mark);
            let label = &col.label;
            caveats.push(format!(
                "{label} is semi-additive over time; the {mark_str} sums it \u{2014} verify aggregation intent."
            ));
        }
    }

    // (b) is_calc percentage measure in bar (stacked/summed) — warn that summing a pct is not meaningful
    if matches!(mark, Mark::Bar) {
        for col in rp.columns.iter().filter(|c| c.role == Role::Measure && c.is_calc) {
            let label = &col.label;
            caveats.push(format!(
                "{label} is a calculated percentage; summing it across categories is not meaningful."
            ));
        }
    }

    // (c) nominal dimension with cardinality > 25 → clutter caveat
    for col in rp.columns.iter().filter(|c| {
        c.role == Role::Dimension
            && matches!(c.data_type, DataType::Nominal)
            && c.cardinality > HIGH_CARDINALITY
    }) {
        let label = &col.label;
        let card = col.cardinality;
        caveats.push(format!(
            "{label} has {card} categories; the chart will be cluttered \u{2014} consider top-N or a different view."
        ));
    }

    caveats
}

/// Human-readable lowercase mark name for caveat strings.
const fn mark_display(mark: &Mark) -> &'static str {
    match mark {
        Mark::Line => "line",
        Mark::Bar => "bar",
        Mark::Area => "area",
        Mark::Point => "point",
        Mark::Rect => "rect",
        Mark::BigNumber => "big number",
        Mark::Table => "table",
    }
}

/// Condense a [`ResultProfile`] into a [`ProfileSummary`].
fn condense_profile(rp: &ResultProfile) -> ProfileSummary {
    let measures = rp
        .columns
        .iter()
        .filter(|c| c.role == Role::Measure)
        .map(|c| c.label.clone())
        .collect();

    let dimensions = rp
        .columns
        .iter()
        .filter(|c| c.role == Role::Dimension)
        .map(|c| c.label.clone())
        .collect();

    ProfileSummary {
        row_count: rp.row_count,
        measures,
        dimensions,
    }
}

// ─── unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn revenue_by_year_inputs() -> (Value, Value) {
        let response = json!({
            "rows": [
                {"revenue": 100.0, "year": "2021"},
                {"revenue": 200.0, "year": "2022"},
                {"revenue": 150.0, "year": "2023"}
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
    fn test_title_one_measure_one_dim() {
        let (response, catalog) = revenue_by_year_inputs();
        let asset = build_asset(&response, &catalog).expect("build_asset failed");
        assert_eq!(asset.title, "Revenue by Year");
    }

    #[test]
    fn test_asset_schema_tag() {
        let (response, catalog) = revenue_by_year_inputs();
        let asset = build_asset(&response, &catalog).expect("build_asset failed");
        assert_eq!(asset.asset, "bi-asset.v1");
    }

    #[test]
    fn test_malformed_returns_error() {
        let bad = json!({"no_rows": true});
        let catalog = json!({"columns": []});
        let result = build_asset(&bad, &catalog);
        assert!(result.is_err());
    }
}
