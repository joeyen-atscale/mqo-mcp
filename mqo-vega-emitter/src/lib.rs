//! mqo-vega-emitter — emit a valid Vega-Lite v5 spec from a chart recommendation + rows.
//!
//! # Usage
//!
//! ```rust
//! use mqo_vega_emitter::emit;
//! use serde_json::json;
//!
//! let rec = json!({
//!     "mark": "Line",
//!     "encoding": {
//!         "x": { "field": "year", "data_type": "temporal" },
//!         "y": { "field": "revenue", "data_type": "quantitative" }
//!     }
//! });
//! let rows = vec![json!({"year": "2023", "revenue": 100})];
//! let spec = emit(&rec, &rows).unwrap();
//! assert_eq!(spec["mark"], "line");
//! ```
//!
//! # Optional `render` feature
//!
//! Enabling the `render` Cargo feature exposes [`render`] — a render-verification
//! module that pipes emitted specs through the `vl-convert` CLI and confirms they
//! produce non-empty SVG/PNG output.  **This feature is for CI/release-gate use
//! only** — it requires `vl-convert` on `PATH` at runtime and has no compile-time
//! dependency of its own.  The default build is unchanged.

#![cfg_attr(not(test), forbid(unsafe_code))]

/// Render-verification capability — only compiled when the `render` Cargo feature is enabled.
///
/// See [`render::render_check`] and [`render::corpus_render_gate`] for entry points.
#[cfg(feature = "render")]
pub mod render;

use serde_json::{Map, Value};
use std::collections::HashSet;
use thiserror::Error;

/// The Vega-Lite v5 `$schema` URL — pinned, not guessed.
pub const VL5_SCHEMA: &str = "https://vega.github.io/schema/vega-lite/v5.json";

/// Structured error type for emission failures.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EmitError {
    /// The recommendation JSON is malformed or missing required fields.
    #[error("malformed recommendation: {0}")]
    MalformedRecommendation(String),

    /// An encoding channel references a field absent from every row.
    ///
    /// `channel` is the encoding channel name (e.g. `"x"`, `"y"`).
    /// `field` is the referenced field name that could not be found.
    #[error("encoding channel `{channel}` references field `{field}` absent from every row")]
    MissingField {
        /// The encoding channel name.
        channel: String,
        /// The missing field name.
        field: String,
    },
}

/// Emit a Vega-Lite v5 spec (JSON) from a chart recommendation and the rows.
///
/// Data is embedded inline under `data.values`. The mark + encoding come from
/// the recommendation. Returns a [`serde_json::Value`] that is a valid VL5 spec.
///
/// # Emission rules
///
/// - Every `quantitative` channel gets `"aggregate": "sum"` by default.
/// - A channel with `"semi_additive": true` does NOT get an aggregate.
/// - `BigNumber` mark emits a `"text"`-mark spec with `encoding.text` set.
/// - `Table` mark emits a `"text"`-mark spec with a top-level `"_render": "table"` key.
///
/// # Errors
///
/// Returns [`EmitError::MalformedRecommendation`] if the recommendation is missing
/// required structure, or [`EmitError::MissingField`] if any encoding channel
/// references a field not present in any row.
pub fn emit(recommendation: &Value, rows: &[Value]) -> Result<Value, EmitError> {
    let mark_str = recommendation
        .get("mark")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            EmitError::MalformedRecommendation("missing or non-string `mark` field".to_owned())
        })?;

    let (vl_mark, is_table) = map_mark(mark_str)?;
    let is_big_number = mark_str == "BigNumber";

    let encoding_obj = recommendation.get("encoding").and_then(Value::as_object);
    let row_fields = collect_row_fields(rows);

    let vl_encoding = if is_big_number {
        build_bignumber_encoding(encoding_obj, &row_fields)?
    } else {
        build_standard_encoding(encoding_obj, &row_fields)?
    };

    Ok(build_spec(vl_mark, is_table, rows, vl_encoding))
}

/// Collect all field names present across all rows.
fn collect_row_fields(rows: &[Value]) -> HashSet<String> {
    rows.iter()
        .filter_map(Value::as_object)
        .flat_map(Map::keys)
        .cloned()
        .collect()
}

/// Build the top-level VL5 spec object with stable field order.
fn build_spec(vl_mark: &str, is_table: bool, rows: &[Value], encoding: Map<String, Value>) -> Value {
    let mut spec = Map::new();
    spec.insert("$schema".to_owned(), Value::String(VL5_SCHEMA.to_owned()));
    spec.insert("data".to_owned(), serde_json::json!({ "values": rows }));
    spec.insert("mark".to_owned(), Value::String(vl_mark.to_owned()));
    spec.insert("encoding".to_owned(), Value::Object(encoding));
    if is_table {
        spec.insert("_render".to_owned(), Value::String("table".to_owned()));
    }
    Value::Object(spec)
}

/// Build encoding for `BigNumber` mark: single `text` channel from any channel entry.
fn build_bignumber_encoding(
    encoding_obj: Option<&Map<String, Value>>,
    row_fields: &HashSet<String>,
) -> Result<Map<String, Value>, EmitError> {
    let mut vl_encoding = Map::new();

    let Some(enc) = encoding_obj else {
        return Ok(vl_encoding);
    };

    let channel_obj = enc
        .get("text")
        .or_else(|| enc.values().next())
        .and_then(Value::as_object);

    let Some(channel_obj) = channel_obj else {
        return Ok(vl_encoding);
    };

    let field = channel_obj
        .get("field")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            EmitError::MalformedRecommendation("BigNumber encoding channel missing `field`".to_owned())
        })?;

    if !row_fields.is_empty() && !row_fields.contains(field) {
        return Err(EmitError::MissingField {
            channel: "text".to_owned(),
            field: field.to_owned(),
        });
    }

    let data_type = channel_obj
        .get("data_type")
        .and_then(Value::as_str)
        .unwrap_or("quantitative");

    let is_semi_additive = channel_obj
        .get("semi_additive")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    vl_encoding.insert("text".to_owned(), build_channel_entry(field, data_type, is_semi_additive));
    Ok(vl_encoding)
}

/// Build encoding for standard marks (Line, Bar, Point, Area, Rect, Table).
fn build_standard_encoding(
    encoding_obj: Option<&Map<String, Value>>,
    row_fields: &HashSet<String>,
) -> Result<Map<String, Value>, EmitError> {
    let mut vl_encoding = Map::new();

    let Some(enc) = encoding_obj else {
        return Ok(vl_encoding);
    };

    for (channel_name, channel_val) in enc {
        let channel_obj = channel_val.as_object().ok_or_else(|| {
            EmitError::MalformedRecommendation(format!(
                "encoding channel `{channel_name}` is not an object"
            ))
        })?;

        let field = channel_obj
            .get("field")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                EmitError::MalformedRecommendation(format!(
                    "encoding channel `{channel_name}` missing `field`"
                ))
            })?;

        if !row_fields.is_empty() && !row_fields.contains(field) {
            return Err(EmitError::MissingField {
                channel: channel_name.clone(),
                field: field.to_owned(),
            });
        }

        let data_type = channel_obj
            .get("data_type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                EmitError::MalformedRecommendation(format!(
                    "encoding channel `{channel_name}` missing `data_type`"
                ))
            })?;

        let is_semi_additive = channel_obj
            .get("semi_additive")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        vl_encoding.insert(channel_name.clone(), build_channel_entry(field, data_type, is_semi_additive));
    }

    Ok(vl_encoding)
}

/// Build a single encoding channel entry `{field, type, [aggregate]}`.
fn build_channel_entry(field: &str, data_type: &str, is_semi_additive: bool) -> Value {
    let mut entry = Map::new();
    entry.insert("field".to_owned(), Value::String(field.to_owned()));
    entry.insert("type".to_owned(), Value::String(data_type.to_owned()));
    if data_type == "quantitative" && !is_semi_additive {
        entry.insert("aggregate".to_owned(), Value::String("sum".to_owned()));
    }
    Value::Object(entry)
}

/// Map a recommendation mark string to the VL5 mark string.
///
/// Returns `(vl_mark, is_table)`.
fn map_mark(mark: &str) -> Result<(&'static str, bool), EmitError> {
    match mark {
        "Line" => Ok(("line", false)),
        "Bar" => Ok(("bar", false)),
        "Point" => Ok(("point", false)),
        "Area" => Ok(("area", false)),
        "Rect" => Ok(("rect", false)),
        "BigNumber" => Ok(("text", false)),
        "Table" => Ok(("text", true)),
        other => Err(EmitError::MalformedRecommendation(format!(
            "unknown mark type: `{other}`"
        ))),
    }
}
