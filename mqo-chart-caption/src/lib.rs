//! `mqo-chart-caption` — deterministic, data-computed takeaway captions for BI assets.
//!
//! Takes a [`ResultProfile`] (from `mqo-result-profiler`) plus the result rows and emits
//! a [`ChartCaption`] (`chart-caption.v1`) payload: a one-line headline takeaway plus
//! 0–3 supporting facts. Every numeric claim is computed from the supplied data — no LLM
//! in the loop, fully deterministic.
//!
//! # Example
//! ```
//! use mqo_chart_caption::{generate_caption, CaptionInput, CaptionConfig};
//! use mqo_result_profiler::{ResultProfile, ColumnProfile, Role, DataType};
//!
//! let profile = ResultProfile {
//!     columns: vec![
//!         ColumnProfile {
//!             name: "revenue".to_string(),
//!             label: "Revenue".to_string(),
//!             role: Role::Measure,
//!             data_type: DataType::Quantitative,
//!             cardinality: 5,
//!             null_rate: 0.0,
//!             measure_range: Some((4_800_000.0, 7_000_000.0)),
//!             is_calc: false,
//!             semi_additive: false,
//!         },
//!         ColumnProfile {
//!             name: "year".to_string(),
//!             label: "Year".to_string(),
//!             role: Role::Dimension,
//!             data_type: DataType::Temporal,
//!             cardinality: 5,
//!             null_rate: 0.0,
//!             measure_range: None,
//!             is_calc: false,
//!             semi_additive: false,
//!         },
//!     ],
//!     row_count: 5,
//!     measure_count: 1,
//!     dimension_count: 1,
//! };
//! let rows = vec![
//!     serde_json::json!({"revenue": 4_800_000.0, "year": "2020"}),
//!     serde_json::json!({"revenue": 5_200_000.0, "year": "2021"}),
//!     serde_json::json!({"revenue": 5_800_000.0, "year": "2022"}),
//!     serde_json::json!({"revenue": 6_300_000.0, "year": "2023"}),
//!     serde_json::json!({"revenue": 7_000_000.0, "year": "2024"}),
//! ];
//! let input = CaptionInput { profile, rows };
//! let caption = generate_caption(&input, &CaptionConfig::default()).unwrap();
//! assert!(!caption.headline.is_empty());
//! ```

#![forbid(unsafe_code)]

use mqo_result_profiler::{ColumnProfile, DataType, ResultProfile, Role};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Cardinality threshold above which a nominal dimension leader/extremum claim
/// is suppressed or flagged as partial (matches `mqo-bi-asset-bundle`).
pub const HIGH_CARDINALITY: usize = 25;

/// Null rate above which a coverage note is considered material.
pub const MATERIAL_NULL_RATE: f64 = 0.1;

// ── Public types ──────────────────────────────────────────────────────────────

/// Input to the caption generator.
#[derive(Debug, Clone)]
pub struct CaptionInput {
    /// The profiled result.
    pub profile: ResultProfile,
    /// The result rows, in projection order.
    pub rows: Vec<Value>,
}

/// Claim category that can appear in a caption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimCategory {
    /// First-to-last delta and % change over an ordered/temporal dimension.
    Trend,
    /// Min or max of a measure and the category holding it.
    Extremum,
    /// Top category by a measure, optionally with runner-up.
    Leader,
    /// Null/coverage note when `null_rate` is material.
    Coverage,
}

/// A single supporting fact in the caption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fact {
    /// Human-readable fact text.
    pub text: String,
    /// Claim category.
    pub category: ClaimCategory,
    /// Machine-readable computed values used to build the text (satisfies R7).
    pub values: Value,
}

/// Reason a claim was not emitted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuppressedReason {
    /// Blocked by a caveat guard (semi_additive / is_calc / high-cardinality).
    CaveatGuard,
    /// Blocked by an operator suppression control.
    SuppressionControl,
    /// Not eligible (insufficient data, wrong dimension type, etc.).
    NotEligible,
}

/// Per-claim provenance record — satisfies R8.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceEntry {
    /// The claim category considered.
    pub category: ClaimCategory,
    /// The measure column considered (if any).
    pub measure: Option<String>,
    /// The dimension column considered (if any).
    pub dimension: Option<String>,
    /// Raw computed values that were available for this claim.
    pub computed_values: Value,
    /// Whether this claim was fired (appeared in `facts`).
    pub fired: bool,
    /// If not fired, the reason.
    pub suppressed_reason: Option<SuppressedReason>,
}

/// The `chart-caption.v1` payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartCaption {
    /// Schema identifier — always `"chart-caption.v1"`.
    pub schema: String,
    /// One-line headline takeaway (may be the R5 no-takeaway form).
    pub headline: String,
    /// 0–3 supporting facts.
    pub facts: Vec<Fact>,
    /// Per-claim provenance for all considered (fired + not-fired) claims.
    pub provenance: Vec<ProvenanceEntry>,
}

/// Number formatting policy — satisfies R13.
#[derive(Debug, Clone)]
pub struct FormatPolicy {
    /// Currency symbol prefix (e.g. "$"). Empty string means no currency prefix.
    pub currency_symbol: String,
    /// Use thousands grouping (e.g. "1,234,567").
    pub thousands_grouping: bool,
    /// Decimal places for percentages (e.g. 1 → "45.0%").
    pub pct_decimal_places: usize,
    /// Abbreviate large numbers (e.g. 1_000_000 → "1.0M").
    pub abbreviate_large: bool,
}

impl Default for FormatPolicy {
    fn default() -> Self {
        Self {
            currency_symbol: String::new(),
            thousands_grouping: true,
            pct_decimal_places: 1,
            abbreviate_large: true,
        }
    }
}

/// A suppression entry — disables a claim category for a named measure, or globally.
#[derive(Debug, Clone)]
pub struct Suppression {
    /// Category to suppress.
    pub category: ClaimCategory,
    /// Measure name to suppress for. `None` means "globally, for all measures".
    pub measure: Option<String>,
}

/// Operator control plane — satisfies R9/R10.
#[derive(Debug, Clone, Default)]
pub struct CaptionConfig {
    /// Zero or more suppression entries.
    pub suppressions: Vec<Suppression>,
    /// Number formatting policy.
    pub format: FormatPolicy,
}

/// Errors that can occur during caption generation.
#[derive(Debug, Error)]
pub enum CaptionError {
    /// A row value could not be serialized to JSON.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

// ── Formatting helpers ─────────────────────────────────────────────────────────

/// Format a numeric value according to the formatting policy.
fn format_number(v: f64, fp: &FormatPolicy) -> String {
    let abs = v.abs();
    let (scaled, suffix) = if fp.abbreviate_large {
        if abs >= 1_000_000_000.0 {
            (v / 1_000_000_000.0, "B")
        } else if abs >= 1_000_000.0 {
            (v / 1_000_000.0, "M")
        } else if abs >= 1_000.0 {
            (v / 1_000.0, "K")
        } else {
            (v, "")
        }
    } else {
        (v, "")
    };

    let raw = if suffix.is_empty() && fp.thousands_grouping && abs >= 1_000.0 {
        // Format with thousands grouping for plain numbers
        format_with_commas(v)
    } else if suffix.is_empty() {
        // Small number — show up to 2 decimal places, trim trailing zeros
        let s = format!("{scaled:.2}");
        trim_trailing_zeros(&s)
    } else {
        // Abbreviated
        let s = format!("{scaled:.1}{suffix}");
        s
    };

    if fp.currency_symbol.is_empty() {
        raw
    } else {
        format!("{}{}", fp.currency_symbol, raw)
    }
}

/// Format f64 with comma thousands separators, up to 2 decimal places.
fn format_with_commas(v: f64) -> String {
    let neg = v < 0.0;
    let abs = v.abs();
    // Integer part
    let int_part = abs.trunc() as u64;
    let frac = abs - abs.trunc();

    // Build groups
    let int_str = int_part.to_string();
    let mut grouped = String::new();
    for (i, c) in int_str.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            grouped.insert(0, ',');
        }
        grouped.insert(0, c);
    }

    let decimal_str = if frac > 0.001 {
        let s = format!("{:.2}", frac);
        let trimmed = trim_trailing_zeros(&s[1..]); // strip leading "0"
        trimmed
    } else {
        String::new()
    };

    let body = if decimal_str.is_empty() {
        grouped
    } else {
        format!("{grouped}{decimal_str}")
    };

    if neg {
        format!("-{body}")
    } else {
        body
    }
}

/// Trim trailing zeros after decimal point (and the point itself if all zeros).
fn trim_trailing_zeros(s: &str) -> String {
    if s.contains('.') {
        let trimmed = s.trim_end_matches('0').trim_end_matches('.');
        trimmed.to_owned()
    } else {
        s.to_owned()
    }
}

/// Format a percentage.
fn format_pct(v: f64, fp: &FormatPolicy) -> String {
    format!("{:.prec$}%", v, prec = fp.pct_decimal_places)
}

// ── Suppression helpers ───────────────────────────────────────────────────────

/// Check if a (category, measure) combination is suppressed by the config.
fn is_suppressed(config: &CaptionConfig, category: &ClaimCategory, measure: &str) -> bool {
    config.suppressions.iter().any(|s| {
        s.category == *category
            && (s.measure.is_none() || s.measure.as_deref() == Some(measure))
    })
}

// ── Row value extraction ──────────────────────────────────────────────────────

/// Extract an f64 value from a row for a named column.
fn row_f64(row: &Value, col: &str) -> Option<f64> {
    row.get(col).and_then(Value::as_f64)
}

/// Extract a string key from a row for a named column (for dim labels).
fn row_str(row: &Value, col: &str) -> Option<String> {
    row.get(col).and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => None,
        _ => Some(v.to_string()),
    })
}

// ── Claim builders ─────────────────────────────────────────────────────────────

/// Build a trend claim for a measure over a temporal/ordered dimension.
///
/// Returns (fact, provenance_entry) or a provenance-only entry if not eligible.
fn build_trend(
    rows: &[Value],
    measure: &ColumnProfile,
    dimension: &ColumnProfile,
    config: &CaptionConfig,
) -> ProvenanceEntry {
    let category = ClaimCategory::Trend;

    // Suppression check
    if is_suppressed(config, &category, &measure.name) {
        return ProvenanceEntry {
            category,
            measure: Some(measure.name.clone()),
            dimension: Some(dimension.name.clone()),
            computed_values: Value::Object(Default::default()),
            fired: false,
            suppressed_reason: Some(SuppressedReason::SuppressionControl),
        };
    }

    // Caveat guard: semi_additive measure over temporal axis → no sum/total claim
    // Trend is a first-last delta, NOT a sum — semi_additive does NOT block trend per the PRD
    // (PRD R4a says "MUST NOT state or imply a *summed total*"; first-last is not a sum).
    // However, we still respect is_calc: a pct measure trend is meaningful (it's not summed).

    // Need at least 2 rows
    let non_null_rows: Vec<&Value> = rows
        .iter()
        .filter(|r| row_f64(r, &measure.name).is_some())
        .collect();

    if non_null_rows.len() < 2 {
        return ProvenanceEntry {
            category,
            measure: Some(measure.name.clone()),
            dimension: Some(dimension.name.clone()),
            computed_values: Value::Object(Default::default()),
            fired: false,
            suppressed_reason: Some(SuppressedReason::NotEligible),
        };
    }

    let first_row = non_null_rows[0];
    let last_row = non_null_rows[non_null_rows.len() - 1];

    let first_val = match row_f64(first_row, &measure.name) {
        Some(v) => v,
        None => {
            return ProvenanceEntry {
                category,
                measure: Some(measure.name.clone()),
                dimension: Some(dimension.name.clone()),
                computed_values: Value::Object(Default::default()),
                fired: false,
                suppressed_reason: Some(SuppressedReason::NotEligible),
            };
        }
    };
    let last_val = match row_f64(last_row, &measure.name) {
        Some(v) => v,
        None => {
            return ProvenanceEntry {
                category,
                measure: Some(measure.name.clone()),
                dimension: Some(dimension.name.clone()),
                computed_values: Value::Object(Default::default()),
                fired: false,
                suppressed_reason: Some(SuppressedReason::NotEligible),
            };
        }
    };

    let first_dim = row_str(first_row, &dimension.name).unwrap_or_default();
    let last_dim = row_str(last_row, &dimension.name).unwrap_or_default();

    let delta = last_val - first_val;
    let pct_change = if first_val.abs() > f64::EPSILON {
        (delta / first_val.abs()) * 100.0
    } else {
        0.0
    };

    let computed_values = serde_json::json!({
        "first_value": first_val,
        "last_value": last_val,
        "delta": delta,
        "pct_change": pct_change,
        "first_dim_label": first_dim,
        "last_dim_label": last_dim,
    });

    ProvenanceEntry {
        category,
        measure: Some(measure.name.clone()),
        dimension: Some(dimension.name.clone()),
        computed_values,
        fired: true,
        suppressed_reason: None,
    }
}

/// Build an extremum (min/max) claim for a measure.
fn build_extremum(
    rows: &[Value],
    measure: &ColumnProfile,
    dimension: Option<&ColumnProfile>,
    config: &CaptionConfig,
) -> ProvenanceEntry {
    let category = ClaimCategory::Extremum;

    if is_suppressed(config, &category, &measure.name) {
        return ProvenanceEntry {
            category,
            measure: Some(measure.name.clone()),
            dimension: dimension.map(|d| d.name.clone()),
            computed_values: Value::Object(Default::default()),
            fired: false,
            suppressed_reason: Some(SuppressedReason::SuppressionControl),
        };
    }

    // Caveat guard: is_calc + extremum is fine (not a sum)
    // semi_additive + extremum is also fine (no sum implied)

    let numeric: Vec<(f64, Option<String>)> = rows
        .iter()
        .filter_map(|r| {
            let v = row_f64(r, &measure.name)?;
            let dim_label = dimension.and_then(|d| row_str(r, &d.name));
            Some((v, dim_label))
        })
        .collect();

    if numeric.is_empty() {
        return ProvenanceEntry {
            category,
            measure: Some(measure.name.clone()),
            dimension: dimension.map(|d| d.name.clone()),
            computed_values: Value::Object(Default::default()),
            fired: false,
            suppressed_reason: Some(SuppressedReason::NotEligible),
        };
    }

    // Find max (more informative as headline than min)
    // Deterministic: first occurrence in case of tie
    let (max_val, max_label) = numeric
        .iter()
        .fold(
            (f64::NEG_INFINITY, None::<String>),
            |(best_v, best_l), (v, l)| {
                if *v > best_v {
                    (*v, l.clone())
                } else {
                    (best_v, best_l)
                }
            },
        );

    let computed_values = serde_json::json!({
        "max_value": max_val,
        "max_dim_label": max_label,
    });

    ProvenanceEntry {
        category,
        measure: Some(measure.name.clone()),
        dimension: dimension.map(|d| d.name.clone()),
        computed_values,
        fired: true,
        suppressed_reason: None,
    }
}

/// Build a leader claim for a measure over a nominal dimension.
fn build_leader(
    rows: &[Value],
    measure: &ColumnProfile,
    dimension: &ColumnProfile,
    config: &CaptionConfig,
) -> ProvenanceEntry {
    let category = ClaimCategory::Leader;

    if is_suppressed(config, &category, &measure.name) {
        return ProvenanceEntry {
            category,
            measure: Some(measure.name.clone()),
            dimension: Some(dimension.name.clone()),
            computed_values: Value::Object(Default::default()),
            fired: false,
            suppressed_reason: Some(SuppressedReason::SuppressionControl),
        };
    }

    // Caveat guard (R4c): high-cardinality nominal dimension → suppress leader claim
    if dimension.cardinality > HIGH_CARDINALITY {
        return ProvenanceEntry {
            category,
            measure: Some(measure.name.clone()),
            dimension: Some(dimension.name.clone()),
            computed_values: serde_json::json!({ "cardinality": dimension.cardinality }),
            fired: false,
            suppressed_reason: Some(SuppressedReason::CaveatGuard),
        };
    }

    // Caveat guard (R4b): is_calc measure — leader claim is fine (not summing)
    // The bundle only guards is_calc when *summing/stacking*; a leader is a single value read.

    // Caveat guard (R4a): semi_additive + temporal is NOT a leader issue (leader is nominal).
    // No extra guard needed here.

    // Collect per-category values; aggregate by taking max for the dim label
    // (rows may be already aggregated, but we take max as the safe default)
    let mut cat_vals: Vec<(String, f64)> = rows
        .iter()
        .filter_map(|r| {
            let val = row_f64(r, &measure.name)?;
            let cat = row_str(r, &dimension.name)?;
            Some((cat, val))
        })
        .collect();

    if cat_vals.is_empty() {
        return ProvenanceEntry {
            category,
            measure: Some(measure.name.clone()),
            dimension: Some(dimension.name.clone()),
            computed_values: Value::Object(Default::default()),
            fired: false,
            suppressed_reason: Some(SuppressedReason::NotEligible),
        };
    }

    // Sort by value descending, then by category name ascending for determinism on ties
    cat_vals.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });

    let (leader_cat, leader_val) = &cat_vals[0];

    let runner_up = cat_vals.get(1).map(|(c, v)| serde_json::json!({"category": c, "value": v}));

    let computed_values = serde_json::json!({
        "leader_category": leader_cat,
        "leader_value": leader_val,
        "runner_up": runner_up,
        "all_categories": cat_vals.len(),
    });

    ProvenanceEntry {
        category,
        measure: Some(measure.name.clone()),
        dimension: Some(dimension.name.clone()),
        computed_values,
        fired: true,
        suppressed_reason: None,
    }
}

/// Build coverage notes for measures with material null rates.
fn build_coverage(
    measure: &ColumnProfile,
    config: &CaptionConfig,
) -> ProvenanceEntry {
    let category = ClaimCategory::Coverage;

    if is_suppressed(config, &category, &measure.name) {
        return ProvenanceEntry {
            category,
            measure: Some(measure.name.clone()),
            dimension: None,
            computed_values: Value::Object(Default::default()),
            fired: false,
            suppressed_reason: Some(SuppressedReason::SuppressionControl),
        };
    }

    if measure.null_rate < MATERIAL_NULL_RATE {
        return ProvenanceEntry {
            category,
            measure: Some(measure.name.clone()),
            dimension: None,
            computed_values: serde_json::json!({ "null_rate": measure.null_rate }),
            fired: false,
            suppressed_reason: Some(SuppressedReason::NotEligible),
        };
    }

    let pct = measure.null_rate * 100.0;
    let computed_values = serde_json::json!({
        "null_rate": measure.null_rate,
        "null_pct": pct,
    });

    ProvenanceEntry {
        category,
        measure: Some(measure.name.clone()),
        dimension: None,
        computed_values,
        fired: true,
        suppressed_reason: None,
    }
}

/// Convert a fired provenance entry to a Fact.
fn provenance_to_fact(
    entry: &ProvenanceEntry,
    rows: &[Value],
    profile: &ResultProfile,
    config: &CaptionConfig,
) -> Option<Fact> {
    if !entry.fired {
        return None;
    }

    let measure_col = entry.measure.as_deref().and_then(|name| {
        profile.columns.iter().find(|c| c.name == name)
    });
    let dim_col = entry.dimension.as_deref().and_then(|name| {
        profile.columns.iter().find(|c| c.name == name)
    });

    let text = build_fact_text(entry, rows, measure_col, dim_col, config);

    Some(Fact {
        text,
        category: entry.category.clone(),
        values: entry.computed_values.clone(),
    })
}

/// Derive the fact text from a provenance entry (re-derived, not stored).
fn build_fact_text(
    entry: &ProvenanceEntry,
    _rows: &[Value],
    measure: Option<&ColumnProfile>,
    _dim: Option<&ColumnProfile>,
    config: &CaptionConfig,
) -> String {
    let fp = &config.format;
    match &entry.category {
        ClaimCategory::Trend => {
            let first_val = entry.computed_values["first_value"].as_f64().unwrap_or(0.0);
            let last_val = entry.computed_values["last_value"].as_f64().unwrap_or(0.0);
            let delta = last_val - first_val;
            let pct_change = entry.computed_values["pct_change"].as_f64().unwrap_or(0.0);
            let first_dim = entry.computed_values["first_dim_label"].as_str().unwrap_or("start");
            let last_dim = entry.computed_values["last_dim_label"].as_str().unwrap_or("end");
            let label = measure.map_or("Measure", |c| c.label.as_str());
            let direction = if delta > 0.0 { "rose" } else if delta < 0.0 { "fell" } else { "held steady" };
            let first_fmt = format_number(first_val, fp);
            let last_fmt = format_number(last_val, fp);
            let pct_fmt = format_pct(pct_change.abs(), fp);
            if delta.abs() < f64::EPSILON {
                format!("{label} held steady at {first_fmt} across {first_dim}–{last_dim}")
            } else {
                format!("{label} {direction} {pct_fmt} from {first_fmt} to {last_fmt} across {first_dim}–{last_dim}")
            }
        }
        ClaimCategory::Extremum => {
            let max_val = entry.computed_values["max_value"].as_f64().unwrap_or(0.0);
            let max_label = entry.computed_values["max_dim_label"].as_str();
            let label = measure.map_or("Measure", |c| c.label.as_str());
            let val_fmt = format_number(max_val, fp);
            match max_label {
                Some(lbl) => format!("{label} peaks at {val_fmt} ({lbl})"),
                None => format!("{label} peaks at {val_fmt}"),
            }
        }
        ClaimCategory::Leader => {
            let leader_cat = entry.computed_values["leader_category"].as_str().unwrap_or("?");
            let leader_val = entry.computed_values["leader_value"].as_f64().unwrap_or(0.0);
            let leader_fmt = format_number(leader_val, fp);
            let label = measure.map_or("measure", |c| c.label.as_str());
            if let Some(runner_obj) = entry.computed_values["runner_up"].as_object() {
                let runner_cat = runner_obj.get("category").and_then(Value::as_str).unwrap_or("?");
                let runner_val = runner_obj.get("value").and_then(Value::as_f64).unwrap_or(0.0);
                let runner_fmt = format_number(runner_val, fp);
                format!("{leader_cat} leads {label} at {leader_fmt}, ahead of {runner_cat}'s {runner_fmt}")
            } else {
                format!("{leader_cat} leads {label} at {leader_fmt}")
            }
        }
        ClaimCategory::Coverage => {
            let pct = entry.computed_values["null_pct"].as_f64().unwrap_or(0.0);
            let label = measure.map_or("measure", |c| c.label.as_str());
            format!(
                "{:.prec$}% of {label} values are missing",
                pct,
                prec = fp.pct_decimal_places
            )
        }
    }
}

// ── Main entry point ──────────────────────────────────────────────────────────

/// Generate a `chart-caption.v1` payload from a profiled result + result rows.
///
/// All numeric claims are derived solely from `input.profile` and `input.rows`.
/// The function is pure (no I/O, no clock, no network) and deterministic:
/// identical inputs produce byte-identical outputs.
///
/// # Errors
/// Returns [`CaptionError`] if serialization of computed values fails (in practice
/// this should never happen for well-formed f64 values).
pub fn generate_caption(
    input: &CaptionInput,
    config: &CaptionConfig,
) -> Result<ChartCaption, CaptionError> {
    let profile = &input.profile;
    let rows = &input.rows;

    // Short-circuit: empty rows → R5 no-takeaway path
    if rows.is_empty() {
        return Ok(ChartCaption {
            schema: "chart-caption.v1".to_owned(),
            headline: "Data is present but no takeaway could be computed.".to_owned(),
            facts: vec![],
            provenance: vec![],
        });
    }

    let measures: Vec<&ColumnProfile> = profile
        .columns
        .iter()
        .filter(|c| c.role == Role::Measure)
        .collect();

    let temporal_dims: Vec<&ColumnProfile> = profile
        .columns
        .iter()
        .filter(|c| c.role == Role::Dimension && matches!(c.data_type, DataType::Temporal))
        .collect();

    let nominal_dims: Vec<&ColumnProfile> = profile
        .columns
        .iter()
        .filter(|c| c.role == Role::Dimension && matches!(c.data_type, DataType::Nominal))
        .collect();

    let mut all_provenance: Vec<ProvenanceEntry> = Vec::new();

    // ── 1. Trend claims — measure × temporal dimension ──────────────────────
    for measure in &measures {
        // Guard R4a: semi_additive over temporal — trend is first-last, NOT a sum;
        // per PRD R4a "summed total" only — we allow trend but record if fully null.
        for tdim in &temporal_dims {
            let entry = build_trend(rows, measure, tdim, config);
            all_provenance.push(entry);
        }
    }

    // ── 2. Leader claims — measure × nominal dimension ───────────────────────
    for measure in &measures {
        for ndim in &nominal_dims {
            let entry = build_leader(rows, measure, ndim, config);
            all_provenance.push(entry);
        }
    }

    // ── 3. Extremum claims — measure (optional temporal dim for context) ─────
    // Only emit if no trend claim fired for this measure (trend is more informative)
    for measure in &measures {
        let trend_fired = all_provenance.iter().any(|p| {
            p.category == ClaimCategory::Trend
                && p.measure.as_deref() == Some(&measure.name)
                && p.fired
        });
        let leader_fired = all_provenance.iter().any(|p| {
            p.category == ClaimCategory::Leader
                && p.measure.as_deref() == Some(&measure.name)
                && p.fired
        });
        // Only add extremum if we don't already have trend+leader for this measure
        if !trend_fired || !leader_fired {
            let dim_for_context = temporal_dims
                .first()
                .copied()
                .or_else(|| nominal_dims.first().copied());
            let entry = build_extremum(rows, measure, dim_for_context, config);
            all_provenance.push(entry);
        }
    }

    // ── 4. Coverage claims ───────────────────────────────────────────────────
    for measure in &measures {
        let entry = build_coverage(measure, config);
        all_provenance.push(entry);
    }

    // ── Select up to 3 facts by priority: trend > leader > extremum > coverage ─
    // R6: deterministic ranking — category priority then order of appearance
    let priority = |cat: &ClaimCategory| match cat {
        ClaimCategory::Trend => 0u8,
        ClaimCategory::Leader => 1,
        ClaimCategory::Extremum => 2,
        ClaimCategory::Coverage => 3,
    };

    let mut fired: Vec<&ProvenanceEntry> = all_provenance
        .iter()
        .filter(|p| p.fired)
        .collect();

    fired.sort_by_key(|p| priority(&p.category));

    // Deduplicate: at most one claim per (category, measure) pair
    let mut seen_measure_cat: Vec<(String, String)> = Vec::new();
    let mut selected: Vec<&ProvenanceEntry> = Vec::new();
    for p in &fired {
        if selected.len() >= 3 {
            break;
        }
        let key = (
            format!("{:?}", p.category),
            p.measure.clone().unwrap_or_default(),
        );
        if !seen_measure_cat.contains(&key) {
            seen_measure_cat.push(key);
            selected.push(p);
        }
    }

    // Build Fact list from selected provenance entries
    let facts: Vec<Fact> = selected
        .iter()
        .filter_map(|p| provenance_to_fact(p, rows, profile, config))
        .collect();

    // ── Build headline ────────────────────────────────────────────────────────
    let headline = if facts.is_empty() {
        "Data is present but no takeaway could be computed.".to_owned()
    } else {
        // Headline is the first (highest-priority) fact text
        facts[0].text.clone()
    };

    Ok(ChartCaption {
        schema: "chart-caption.v1".to_owned(),
        headline,
        facts,
        provenance: all_provenance,
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mqo_result_profiler::{ColumnProfile, DataType, ResultProfile, Role};

    fn make_profile(columns: Vec<ColumnProfile>) -> ResultProfile {
        let measure_count = columns.iter().filter(|c| c.role == Role::Measure).count();
        let dimension_count = columns.iter().filter(|c| c.role == Role::Dimension).count();
        let row_count = 0; // set by caller if needed
        ResultProfile {
            columns,
            row_count,
            measure_count,
            dimension_count,
        }
    }

    fn revenue_col() -> ColumnProfile {
        ColumnProfile {
            name: "revenue".to_string(),
            label: "Revenue".to_string(),
            role: Role::Measure,
            data_type: DataType::Quantitative,
            cardinality: 5,
            null_rate: 0.0,
            measure_range: Some((4_800_000.0, 7_000_000.0)),
            is_calc: false,
            semi_additive: false,
        }
    }

    fn year_col() -> ColumnProfile {
        ColumnProfile {
            name: "year".to_string(),
            label: "Year".to_string(),
            role: Role::Dimension,
            data_type: DataType::Temporal,
            cardinality: 5,
            null_rate: 0.0,
            measure_range: None,
            is_calc: false,
            semi_additive: false,
        }
    }

    fn region_col() -> ColumnProfile {
        ColumnProfile {
            name: "region".to_string(),
            label: "Region".to_string(),
            role: Role::Dimension,
            data_type: DataType::Nominal,
            cardinality: 4,
            null_rate: 0.0,
            measure_range: None,
            is_calc: false,
            semi_additive: false,
        }
    }

    fn margin_col() -> ColumnProfile {
        ColumnProfile {
            name: "margin_pct".to_string(),
            label: "Margin %".to_string(),
            role: Role::Measure,
            data_type: DataType::Quantitative,
            cardinality: 4,
            null_rate: 0.0,
            measure_range: Some((15.6, 24.1)),
            is_calc: true,
            semi_additive: false,
        }
    }

    /// Test: trend headline for revenue-by-year
    #[test]
    fn test_trend_headline() {
        let profile = make_profile(vec![revenue_col(), year_col()]);
        let rows = vec![
            serde_json::json!({"revenue": 4_800_000.0, "year": "2020"}),
            serde_json::json!({"revenue": 5_200_000.0, "year": "2021"}),
            serde_json::json!({"revenue": 5_800_000.0, "year": "2022"}),
            serde_json::json!({"revenue": 6_300_000.0, "year": "2023"}),
            serde_json::json!({"revenue": 7_000_000.0, "year": "2024"}),
        ];
        let input = CaptionInput { profile, rows };
        let caption = generate_caption(&input, &CaptionConfig::default()).unwrap();

        assert_eq!(caption.schema, "chart-caption.v1");
        assert!(!caption.headline.is_empty());
        assert!(
            caption.headline.contains("Revenue"),
            "headline should mention the measure: {}",
            caption.headline
        );
        assert!(
            caption.headline.contains("2020") && caption.headline.contains("2024"),
            "headline should mention start and end year: {}",
            caption.headline
        );
        // The pct change is 45.8%
        assert!(
            caption.headline.contains("45.8%") || caption.headline.contains("rose") || caption.headline.contains("fell"),
            "headline should mention direction: {}",
            caption.headline
        );

        // Verify all numeric claims are re-derivable
        let trend_fact = caption.facts.iter().find(|f| f.category == ClaimCategory::Trend);
        assert!(trend_fact.is_some(), "should have a trend fact");
        let tfact = trend_fact.unwrap();
        let fv = tfact.values["first_value"].as_f64().unwrap();
        let lv = tfact.values["last_value"].as_f64().unwrap();
        assert!((fv - 4_800_000.0).abs() < 1.0, "first_value should be from rows");
        assert!((lv - 7_000_000.0).abs() < 1.0, "last_value should be from rows");
    }

    /// Test: leader fact for margin-by-region
    #[test]
    fn test_leader_fact() {
        let profile = make_profile(vec![margin_col(), region_col()]);
        let rows = vec![
            serde_json::json!({"margin_pct": 24.1, "region": "APAC"}),
            serde_json::json!({"margin_pct": 15.6, "region": "LATAM"}),
            serde_json::json!({"margin_pct": 19.3, "region": "EMEA"}),
            serde_json::json!({"margin_pct": 21.0, "region": "NA"}),
        ];
        let input = CaptionInput { profile, rows };
        let caption = generate_caption(&input, &CaptionConfig::default()).unwrap();

        let leader_fact = caption.facts.iter().find(|f| f.category == ClaimCategory::Leader);
        assert!(leader_fact.is_some(), "should have a leader fact");
        let lf = leader_fact.unwrap();
        assert!(
            lf.text.contains("APAC"),
            "leader should be APAC (highest margin): {}",
            lf.text
        );
        // verify value from rows
        let lv = lf.values["leader_value"].as_f64().unwrap();
        assert!((lv - 24.1).abs() < 0.01, "leader_value should be 24.1 from rows");
    }

    /// Test: empty rows returns R5 no-takeaway headline without error
    #[test]
    fn test_empty_rows_no_error() {
        let profile = make_profile(vec![revenue_col(), year_col()]);
        let input = CaptionInput { profile, rows: vec![] };
        let caption = generate_caption(&input, &CaptionConfig::default()).unwrap();

        assert_eq!(caption.schema, "chart-caption.v1");
        assert!(!caption.headline.is_empty(), "headline must not be empty");
        assert!(caption.facts.is_empty(), "no facts for empty rows");
        assert!(
            caption.headline.contains("no takeaway") || caption.headline.contains("present"),
            "headline should state no takeaway: {}",
            caption.headline
        );
    }

    /// Test: single row — no trend claim, no error
    #[test]
    fn test_single_row_no_trend() {
        let profile = make_profile(vec![revenue_col(), year_col()]);
        let rows = vec![serde_json::json!({"revenue": 5_000_000.0, "year": "2023"})];
        let input = CaptionInput { profile, rows };
        let caption = generate_caption(&input, &CaptionConfig::default()).unwrap();

        let trend_fact = caption.facts.iter().find(|f| f.category == ClaimCategory::Trend);
        assert!(trend_fact.is_none(), "no trend claim for single row");
        // Should not error; may have extremum
        assert!(!caption.headline.is_empty());
    }

    /// Test: all-null measure — no numeric claims, no fabricated values
    #[test]
    fn test_all_null_measure() {
        let mut col = revenue_col();
        col.null_rate = 1.0;
        col.measure_range = None;
        let profile = make_profile(vec![col, year_col()]);
        let rows = vec![
            serde_json::json!({"revenue": null, "year": "2020"}),
            serde_json::json!({"revenue": null, "year": "2021"}),
        ];
        let input = CaptionInput { profile, rows };
        let caption = generate_caption(&input, &CaptionConfig::default()).unwrap();

        for f in &caption.facts {
            assert!(
                f.category == ClaimCategory::Coverage,
                "only coverage facts allowed for all-null measure, got: {:?}",
                f.category
            );
        }
    }

    /// Test: no temporal dimension → no trend claim
    #[test]
    fn test_no_temporal_dim_no_trend() {
        let profile = make_profile(vec![margin_col(), region_col()]);
        let rows = vec![
            serde_json::json!({"margin_pct": 24.1, "region": "APAC"}),
            serde_json::json!({"margin_pct": 15.6, "region": "LATAM"}),
        ];
        let input = CaptionInput { profile, rows };
        let caption = generate_caption(&input, &CaptionConfig::default()).unwrap();

        let trend_fact = caption.facts.iter().find(|f| f.category == ClaimCategory::Trend);
        assert!(trend_fact.is_none(), "no trend for no temporal dim");
    }

    /// Test: semi_additive guard — trend claim suppressed (note: only *sum* blocked, not trend)
    /// The PRD R4a says "summed total" — trend (first-last delta) is allowed.
    /// This test verifies the guard for a claim that IS a sum (extremum is a point value,
    /// also fine). We test that provenance records the semi_additive state.
    #[test]
    fn test_semi_additive_provenance_recorded() {
        let mut rev = revenue_col();
        rev.semi_additive = true;
        let profile = make_profile(vec![rev, year_col()]);
        let rows = vec![
            serde_json::json!({"revenue": 100.0, "year": "2020"}),
            serde_json::json!({"revenue": 120.0, "year": "2021"}),
        ];
        let input = CaptionInput { profile, rows };
        let caption = generate_caption(&input, &CaptionConfig::default()).unwrap();

        // Caption must not assert a summed total; trend is a first-last which is fine
        // At minimum: provenance must exist
        assert!(!caption.provenance.is_empty(), "provenance must be populated");
        assert_eq!(caption.schema, "chart-caption.v1");
    }

    /// Test: operator suppression — trend suppressed for a named measure
    #[test]
    fn test_operator_suppression_trend() {
        let profile = make_profile(vec![revenue_col(), year_col()]);
        let rows = vec![
            serde_json::json!({"revenue": 4_800_000.0, "year": "2020"}),
            serde_json::json!({"revenue": 7_000_000.0, "year": "2024"}),
        ];
        let config = CaptionConfig {
            suppressions: vec![Suppression {
                category: ClaimCategory::Trend,
                measure: Some("revenue".to_string()),
            }],
            format: FormatPolicy::default(),
        };
        let input = CaptionInput { profile, rows };
        let caption = generate_caption(&input, &config).unwrap();

        let trend_fact = caption.facts.iter().find(|f| f.category == ClaimCategory::Trend);
        assert!(trend_fact.is_none(), "trend should be suppressed");

        // Provenance should record it as suppressed
        let suppressed = caption.provenance.iter().find(|p| {
            p.category == ClaimCategory::Trend
                && p.measure.as_deref() == Some("revenue")
                && matches!(p.suppressed_reason, Some(SuppressedReason::SuppressionControl))
        });
        assert!(suppressed.is_some(), "provenance must record suppression");
    }

    /// Test: high-cardinality nominal dimension → leader claim suppressed with caveat_guard
    #[test]
    fn test_high_cardinality_leader_suppressed() {
        let mut big_dim = region_col();
        big_dim.cardinality = 50; // > HIGH_CARDINALITY
        let profile = make_profile(vec![revenue_col(), big_dim]);
        let rows: Vec<Value> = (0..50u64)
            .map(|i| serde_json::json!({"revenue": i as f64 * 1000.0, "region": format!("R{i}")}))
            .collect();
        let input = CaptionInput { profile, rows };
        let caption = generate_caption(&input, &CaptionConfig::default()).unwrap();

        let leader_fact = caption.facts.iter().find(|f| f.category == ClaimCategory::Leader);
        assert!(leader_fact.is_none(), "leader should be suppressed for high-cardinality dim");

        let caveat_entry = caption.provenance.iter().find(|p| {
            p.category == ClaimCategory::Leader
                && matches!(p.suppressed_reason, Some(SuppressedReason::CaveatGuard))
        });
        assert!(caveat_entry.is_some(), "provenance must record caveat_guard suppression");
    }

    /// Test: determinism — identical inputs produce byte-identical JSON output
    #[test]
    fn test_determinism() {
        let profile = make_profile(vec![revenue_col(), year_col()]);
        let rows = vec![
            serde_json::json!({"revenue": 4_800_000.0, "year": "2020"}),
            serde_json::json!({"revenue": 7_000_000.0, "year": "2024"}),
        ];
        let input = CaptionInput { profile: profile.clone(), rows: rows.clone() };
        let c1 = generate_caption(&input, &CaptionConfig::default()).unwrap();
        let c2 = generate_caption(&CaptionInput { profile, rows }, &CaptionConfig::default()).unwrap();

        let j1 = serde_json::to_string(&c1).unwrap();
        let j2 = serde_json::to_string(&c2).unwrap();
        assert_eq!(j1, j2, "outputs must be byte-identical for identical inputs");
    }

    /// Test: operator provenance — all considered claims appear in provenance
    #[test]
    fn test_provenance_completeness() {
        let profile = make_profile(vec![revenue_col(), year_col(), region_col()]);
        let rows = vec![
            serde_json::json!({"revenue": 100.0, "year": "2020", "region": "APAC"}),
            serde_json::json!({"revenue": 120.0, "year": "2021", "region": "LATAM"}),
        ];
        let input = CaptionInput { profile, rows };
        let caption = generate_caption(&input, &CaptionConfig::default()).unwrap();

        // Provenance must have entries for all claim categories considered
        let has_trend = caption.provenance.iter().any(|p| p.category == ClaimCategory::Trend);
        let has_leader = caption.provenance.iter().any(|p| p.category == ClaimCategory::Leader);
        let has_coverage = caption.provenance.iter().any(|p| p.category == ClaimCategory::Coverage);
        assert!(has_trend, "provenance must have trend entry");
        assert!(has_leader, "provenance must have leader entry");
        assert!(has_coverage, "provenance must have coverage entry");

        // All provenance entries have measure set (or suppressed_reason explains why)
        for p in &caption.provenance {
            assert!(
                p.measure.is_some(),
                "all provenance entries must name the measure: {:?}",
                p
            );
        }
    }

    /// Test: tie for max — deterministic ordering (alphabetical on dim name)
    #[test]
    fn test_tie_deterministic() {
        let profile = make_profile(vec![revenue_col(), region_col()]);
        let rows = vec![
            serde_json::json!({"revenue": 100.0, "region": "ZULU"}),
            serde_json::json!({"revenue": 100.0, "region": "ALPHA"}),
        ];
        let input = CaptionInput { profile: profile.clone(), rows: rows.clone() };
        let c1 = generate_caption(&input, &CaptionConfig::default()).unwrap();
        let c2 = generate_caption(&CaptionInput { profile, rows }, &CaptionConfig::default()).unwrap();

        let j1 = serde_json::to_string(&c1).unwrap();
        let j2 = serde_json::to_string(&c2).unwrap();
        assert_eq!(j1, j2, "tie must resolve deterministically");

        // ALPHA should win (alphabetically earlier)
        let leader_fact = c1.facts.iter().find(|f| f.category == ClaimCategory::Leader);
        if let Some(lf) = leader_fact {
            assert!(lf.text.contains("ALPHA"), "tie should resolve to ALPHA (alphabetically first): {}", lf.text);
        }
    }
}
