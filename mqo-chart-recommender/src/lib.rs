//! `mqo-chart-recommender` — chart type + encoding recommendation from `result-profile.v1`.
//!
//! Given a typed column inventory (the `result-profile.v1` JSON produced by
//! `mqo-result-profiler`), this crate determines the best chart mark and
//! encoding: `{mark, encoding, rationale, alternatives}` emitted as
//! `chart-recommendation.v1` JSON.
//!
//! # Quick start
//!
//! ```rust
//! use mqo_chart_recommender::{recommend, Mark};
//! use serde_json::json;
//!
//! let profile = json!({
//!     "schema": "result-profile.v1",
//!     "columns": [
//!         {"name": "order_date", "role": "dimension", "is_temporal": true, "cardinality": 365},
//!         {"name": "revenue",    "role": "measure",   "is_temporal": false, "cardinality": null}
//!     ]
//! });
//! let rec = recommend(&profile).unwrap();
//! assert_eq!(rec.mark, Mark::Line);
//! ```

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─── public types ────────────────────────────────────────────────────────────

/// The visual mark chosen for the recommendation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mark {
    /// A single large number (KPI card). Used when there is exactly one measure
    /// and no dimensions.
    BigNumber,
    /// A horizontal or vertical bar chart. Categorical comparison.
    Bar,
    /// A line chart. Best for temporal dimensions.
    Line,
    /// A scatter / bubble plot. Correlation between two measures.
    Point,
    /// An area chart. Filled line; reserved for future v1+ use.
    Area,
    /// A heatmap (Vega-Lite `rect` mark). Two nominal dimensions.
    Rect,
    /// A plain data table — no visual encoding. Fallback when no quantitative
    /// column exists, or when cardinality is too high to plot sensibly.
    Table,
}

/// A single encoding channel: maps a column name to its Vega-Lite data type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channel {
    /// Column name as it appears in the result set.
    pub field: String,
    /// Vega-Lite data type: `"quantitative"`, `"temporal"`, `"nominal"`, or
    /// `"ordinal"`.
    pub data_type: String,
}

/// The full encoding (x / y / color / theta axes).
///
/// `None` channels are omitted from the serialised JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Encoding {
    /// Horizontal axis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<Channel>,
    /// Vertical axis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<Channel>,
    /// Color channel (series, hue).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Channel>,
    /// Theta channel (pie/arc — reserved; always `None` in v1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theta: Option<Channel>,
}

/// A runner-up mark with a one-line reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alternative {
    /// The alternative mark.
    pub mark: Mark,
    /// Why this mark is worth considering.
    pub reason: String,
}

/// The full recommendation: mark + encoding + rationale + ranked alternatives.
///
/// Serialised as `chart-recommendation.v1` JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChartRecommendation {
    /// Schema version tag.
    pub schema: String,
    /// The recommended Vega-Lite mark.
    pub mark: Mark,
    /// Column-to-channel mapping.
    pub encoding: Encoding,
    /// One-sentence explanation for the choice.
    pub rationale: String,
    /// Ranked alternative marks, best-first.
    pub alternatives: Vec<Alternative>,
}

// ─── errors ──────────────────────────────────────────────────────────────────

/// Errors returned by [`recommend`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecommendError {
    /// The JSON value was not an object, or the `columns` array was missing /
    /// had a wrong type.
    MalformedProfile(String),
}

impl std::fmt::Display for RecommendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedProfile(msg) => write!(f, "malformed result-profile.v1: {msg}"),
        }
    }
}

impl std::error::Error for RecommendError {}

// ─── internal column descriptor ──────────────────────────────────────────────

#[derive(Debug)]
struct ColDesc {
    name: String,
    role: Role,
    is_temporal: bool,
    cardinality: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Measure,
    Dimension,
}

// ─── public entry point ──────────────────────────────────────────────────────

/// Recommend a chart type and encoding from a `result-profile.v1` JSON value.
///
/// # Errors
///
/// Returns [`RecommendError::MalformedProfile`] when the JSON is not a valid
/// `result-profile.v1` object (missing `columns` array, unknown `role` string,
/// etc.).
pub fn recommend(profile: &Value) -> Result<ChartRecommendation, RecommendError> {
    let columns = parse_columns(profile)?;
    Ok(decide(&columns))
}

// ─── parsing ─────────────────────────────────────────────────────────────────

fn parse_columns(profile: &Value) -> Result<Vec<ColDesc>, RecommendError> {
    let obj = profile
        .as_object()
        .ok_or_else(|| RecommendError::MalformedProfile("root is not a JSON object".into()))?;

    let cols_val = obj
        .get("columns")
        .ok_or_else(|| RecommendError::MalformedProfile("missing `columns` field".into()))?;

    let cols_arr = cols_val.as_array().ok_or_else(|| {
        RecommendError::MalformedProfile("`columns` is not a JSON array".into())
    })?;

    cols_arr
        .iter()
        .enumerate()
        .map(|(i, v)| parse_column(i, v))
        .collect()
}

fn parse_column(idx: usize, v: &Value) -> Result<ColDesc, RecommendError> {
    let obj = v.as_object().ok_or_else(|| {
        RecommendError::MalformedProfile(format!("column[{idx}] is not an object"))
    })?;

    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RecommendError::MalformedProfile(format!("column[{idx}].name missing or not a string"))
        })?
        .to_owned();

    let role_str = obj
        .get("role")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            RecommendError::MalformedProfile(format!("column[{idx}].role missing or not a string"))
        })?;

    let role = match role_str {
        "measure" => Role::Measure,
        "dimension" => Role::Dimension,
        other => {
            return Err(RecommendError::MalformedProfile(format!(
                "column[{idx}].role unknown value `{other}`"
            )));
        }
    };

    let is_temporal = obj
        .get("is_temporal")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let cardinality = obj
        .get("cardinality")
        .and_then(|c| if c.is_null() { None } else { c.as_u64() });

    Ok(ColDesc {
        name,
        role,
        is_temporal,
        cardinality,
    })
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn quant(field: &str) -> Channel {
    Channel { field: field.to_owned(), data_type: "quantitative".into() }
}

fn temporal(field: &str) -> Channel {
    Channel { field: field.to_owned(), data_type: "temporal".into() }
}

fn nominal(field: &str) -> Channel {
    Channel { field: field.to_owned(), data_type: "nominal".into() }
}

fn rec(mark: Mark, encoding: Encoding, rationale: String, alternatives: Vec<Alternative>) -> ChartRecommendation {
    ChartRecommendation {
        schema: "chart-recommendation.v1".into(),
        mark,
        encoding,
        rationale,
        alternatives,
    }
}

// ─── decision functions ──────────────────────────────────────────────────────

/// High-cardinality threshold: above this, `Table` becomes an alternative.
const HIGH_CARDINALITY: u64 = 25;

fn decide(columns: &[ColDesc]) -> ChartRecommendation {
    let measures: Vec<&ColDesc> = columns.iter().filter(|c| c.role == Role::Measure).collect();
    let dims: Vec<&ColDesc> = columns.iter().filter(|c| c.role == Role::Dimension).collect();
    match (measures.len(), dims.len()) {
        (0, _) => decide_no_measures(),
        (1, 0) => decide_kpi(&measures),
        (1, 1) => decide_one_measure_one_dim(&measures, &dims),
        (2, 0) => decide_scatter(&measures),
        (2, 1) => decide_coloured_scatter(&measures, &dims),
        (1, 2) => decide_one_measure_two_dims(&measures, &dims),
        (nm, nd) => rec(
            Mark::Table,
            Encoding::default(),
            format!("No specific rule matched ({nm} measure(s), {nd} dimension(s)); defaulting to table."),
            vec![],
        ),
    }
}

fn decide_no_measures() -> ChartRecommendation {
    rec(
        Mark::Table,
        Encoding::default(),
        "No quantitative measures present; displaying raw data in a table.".into(),
        vec![],
    )
}

fn decide_kpi(measures: &[&ColDesc]) -> ChartRecommendation {
    let Some(m) = measures.first() else {
        return decide_no_measures();
    };
    rec(
        Mark::BigNumber,
        Encoding { y: Some(quant(&m.name)), ..Encoding::default() },
        format!("Single measure `{}` with no dimensions is best shown as a KPI card.", m.name),
        vec![Alternative { mark: Mark::Table, reason: "Table is always a safe fallback for a single scalar.".into() }],
    )
}

fn decide_one_measure_one_dim(measures: &[&ColDesc], dims: &[&ColDesc]) -> ChartRecommendation {
    let (Some(m), Some(d)) = (measures.first(), dims.first()) else {
        return decide_no_measures();
    };
    if d.is_temporal {
        decide_line_temporal(m, d)
    } else {
        decide_bar_nominal(m, d)
    }
}

fn decide_line_temporal(m: &ColDesc, d: &ColDesc) -> ChartRecommendation {
    rec(
        Mark::Line,
        Encoding {
            x: Some(temporal(&d.name)),
            y: Some(quant(&m.name)),
            ..Encoding::default()
        },
        format!(
            "Temporal dimension `{}` on x with measure `{}` on y forms a time-series line chart.",
            d.name, m.name
        ),
        vec![
            Alternative { mark: Mark::Area, reason: "Area emphasises cumulative trend over a time axis.".into() },
            Alternative { mark: Mark::Bar,  reason: "Bar works for coarse time granularities (year, quarter).".into() },
        ],
    )
}

fn decide_bar_nominal(m: &ColDesc, d: &ColDesc) -> ChartRecommendation {
    let high_card = d.cardinality.is_some_and(|c| c > HIGH_CARDINALITY);

    let rationale = if high_card {
        format!(
            "Nominal dimension `{}` on x with measure `{}` on y; high cardinality ({}) may make the chart crowded — consider the Table alternative.",
            d.name, m.name, d.cardinality.unwrap_or(0)
        )
    } else {
        format!(
            "Nominal dimension `{}` on x with measure `{}` on y; bar chart supports categorical comparison.",
            d.name, m.name
        )
    };

    let mut alternatives = Vec::new();
    if high_card {
        alternatives.push(Alternative {
            mark: Mark::Table,
            reason: format!(
                "Dimension `{}` has {} distinct values; a table is more readable than a crowded bar chart.",
                d.name, d.cardinality.unwrap_or(0)
            ),
        });
    }
    alternatives.push(Alternative {
        mark: Mark::Point,
        reason: "Dot plot works well for ranked nominal data.".into(),
    });

    rec(
        Mark::Bar,
        Encoding {
            x: Some(nominal(&d.name)),
            y: Some(quant(&m.name)),
            ..Encoding::default()
        },
        rationale,
        alternatives,
    )
}

fn decide_scatter(measures: &[&ColDesc]) -> ChartRecommendation {
    let (Some(m0), Some(m1)) = (measures.first(), measures.get(1)) else {
        return decide_no_measures();
    };
    rec(
        Mark::Point,
        Encoding {
            x: Some(quant(&m0.name)),
            y: Some(quant(&m1.name)),
            ..Encoding::default()
        },
        format!(
            "Two measures `{}` vs `{}` with no dimensions — scatter plot shows correlation.",
            m0.name, m1.name
        ),
        vec![Alternative {
            mark: Mark::Table,
            reason: "Table preserves exact values when visual correlation is not needed.".into(),
        }],
    )
}

fn decide_coloured_scatter(measures: &[&ColDesc], dims: &[&ColDesc]) -> ChartRecommendation {
    let (Some(m0), Some(m1), Some(d)) = (measures.first(), measures.get(1), dims.first()) else {
        return decide_no_measures();
    };
    rec(
        Mark::Point,
        Encoding {
            x: Some(quant(&m0.name)),
            y: Some(quant(&m1.name)),
            color: Some(nominal(&d.name)),
            ..Encoding::default()
        },
        format!(
            "Two measures `{}`/`{}` with dimension `{}` as color — coloured scatter reveals per-category correlation.",
            m0.name, m1.name, d.name
        ),
        vec![Alternative {
            mark: Mark::Table,
            reason: "Table is the safe fallback when the scatter becomes over-plotted.".into(),
        }],
    )
}

fn decide_one_measure_two_dims(measures: &[&ColDesc], dims: &[&ColDesc]) -> ChartRecommendation {
    let Some(m) = measures.first() else {
        return decide_no_measures();
    };
    let temporal_dim = dims.iter().find(|d| d.is_temporal).copied();
    let nominal_dims: Vec<&ColDesc> = dims.iter().filter(|d| !d.is_temporal).copied().collect();

    temporal_dim.map_or_else(
        || decide_grouped_bar(m, dims),
        |td| decide_multi_series_line(m, td, &nominal_dims),
    )
}

fn decide_multi_series_line(m: &ColDesc, td: &ColDesc, nominal_dims: &[&ColDesc]) -> ChartRecommendation {
    let color_dim = nominal_dims.first().copied();
    let color_channel = color_dim.map(|cd| nominal(&cd.name));
    let color_note = color_dim.map_or_else(String::new, |cd| format!(", colored by `{}`", cd.name));

    rec(
        Mark::Line,
        Encoding {
            x: Some(temporal(&td.name)),
            y: Some(quant(&m.name)),
            color: color_channel,
            ..Encoding::default()
        },
        format!(
            "Temporal dimension `{}` on x, measure `{}` on y{}; multi-series line chart.",
            td.name, m.name, color_note
        ),
        vec![Alternative {
            mark: Mark::Area,
            reason: "Stacked area shows part-to-whole across series.".into(),
        }],
    )
}

fn decide_grouped_bar(m: &ColDesc, dims: &[&ColDesc]) -> ChartRecommendation {
    let (Some(d0), Some(d1)) = (dims.first(), dims.get(1)) else {
        return decide_no_measures();
    };
    rec(
        Mark::Bar,
        Encoding {
            x: Some(nominal(&d0.name)),
            y: Some(quant(&m.name)),
            color: Some(nominal(&d1.name)),
            ..Encoding::default()
        },
        format!(
            "Two nominal dimensions `{}`/`{}` with measure `{}`; grouped bar allows cross-category comparison.",
            d0.name, d1.name, m.name
        ),
        vec![Alternative {
            mark: Mark::Rect,
            reason: "Heatmap (rect) is more compact when both dimensions have many values.".into(),
        }],
    )
}

// ─── unit tests (supplemental) ───────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_line_for_temporal() {
        let profile = json!({
            "schema": "result-profile.v1",
            "columns": [
                {"name": "order_date", "role": "dimension", "is_temporal": true,  "cardinality": 365},
                {"name": "revenue",    "role": "measure",   "is_temporal": false, "cardinality": null}
            ]
        });
        let rec = recommend(&profile).unwrap();
        assert_eq!(rec.mark, Mark::Line);
        assert_eq!(rec.encoding.x.as_ref().map(|c| c.field.as_str()), Some("order_date"));
        assert_eq!(rec.encoding.y.as_ref().map(|c| c.field.as_str()), Some("revenue"));
    }

    #[test]
    fn test_malformed_missing_columns() {
        let err = recommend(&json!({"schema": "result-profile.v1"})).unwrap_err();
        assert!(matches!(err, RecommendError::MalformedProfile(_)));
    }

    #[test]
    fn test_malformed_not_object() {
        let err = recommend(&json!("not-an-object")).unwrap_err();
        assert!(matches!(err, RecommendError::MalformedProfile(_)));
    }
}
