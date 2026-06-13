//! MQO builder: assembles a validated `BoundMqo` from a resolved projection.

use mqo_catalog_binder::catalog::CatalogSnapshot;
use mqo_spec::{Filter, LevelSelection, MeasureRef, Mqo, BoundDimension, BoundMeasure, BoundMqo};

use crate::error::{MqoFromSqlError, ResolveError};
use crate::parser::{FilterClause, ParsedProjection};
use crate::resolver::{resolve_dimension_level, resolve_measure, ResolvedKind};

/// Build a `BoundMqo` from a `ParsedProjection` using the provided catalog snapshot.
///
/// # Errors
///
/// Returns [`MqoFromSqlError`] on resolve failure or MQO validation failure.
pub fn build_bound_mqo(
    parsed: &ParsedProjection,
    snapshot: &CatalogSnapshot,
) -> Result<BoundMqo, MqoFromSqlError> {
    // Split SELECT columns into measures vs dimensions
    // Names in GROUP BY are dimension levels; names only in SELECT are measures
    let dim_set: std::collections::HashSet<&String> = parsed.dimensions.iter().collect();

    let mut mqo_measures: Vec<MeasureRef> = Vec::new();
    let mut bound_measures: Vec<BoundMeasure> = Vec::new();

    let mut mqo_dimensions: Vec<LevelSelection> = Vec::new();
    let mut bound_dimensions: Vec<BoundDimension> = Vec::new();

    // Resolve SELECT columns
    for name in &parsed.measures {
        if dim_set.contains(name) {
            // Already handled in GROUP BY loop below; skip here
        } else {
            let entry = resolve_measure(name, snapshot).map_err(MqoFromSqlError::Resolve)?;
            mqo_measures.push(MeasureRef {
                unique_name: entry.unique_name.clone(),
            });
            bound_measures.push(BoundMeasure {
                unique_name: entry.unique_name.clone(),
                is_calc: entry.is_calc,
                semi_additive: entry.semi_additive.is_some(),
                required_dimension: entry.required_dimension.clone(),
            });
        }
    }

    // Resolve GROUP BY columns as dimension levels
    for name in &parsed.dimensions {
        let entry = resolve_dimension_level(name, snapshot).map_err(MqoFromSqlError::Resolve)?;
        let hierarchy = entry.hierarchy.clone().unwrap_or_default();
        let level = entry.level.clone().unwrap_or_default();
        mqo_dimensions.push(LevelSelection {
            hierarchy: hierarchy.clone(),
            level,
        });
        bound_dimensions.push(BoundDimension {
            unique_name: entry.unique_name.clone(),
            hierarchy,
        });
    }

    // Build filters from WHERE clauses
    let mqo_filters = build_filters(&parsed.filters, snapshot)?;

    // Assemble the MQO
    let mqo = Mqo {
        model: parsed.model_id.clone(),
        measures: mqo_measures,
        dimensions: mqo_dimensions,
        filters: mqo_filters,
        time_intelligence: vec![],
        order: None,
        limit: parsed.limit,
        non_empty: true,
    };

    // Validate
    mqo_spec::validate(&mqo).map_err(MqoFromSqlError::InvalidMqo)?;

    Ok(BoundMqo {
        mqo,
        measures: bound_measures,
        dimensions: bound_dimensions,
    })
}

/// Convert `FilterClause` list into MQO `Filter` objects.
///
/// Currently supports:
/// - `= <string>` → `Filter::Member { members: [value] }`
/// - `IN [...]` → `Filter::Member { members: [...] }`
/// - `= <number>`, `!=`, `<`, `>`, `<=`, `>=` → `Filter::Range` where applicable
///
/// The filter column is resolved against the catalog snapshot.
fn build_filters(
    clauses: &[FilterClause],
    snapshot: &CatalogSnapshot,
) -> Result<Vec<Filter>, MqoFromSqlError> {
    let mut filters = Vec::new();

    for clause in clauses {
        let col = &clause.col;

        // Resolve the column — it should be a dimension level or measure
        let resolved_kind = resolve_dimension_level(col, snapshot)
            .map(|e| ResolvedKind::DimensionLevel {
                hierarchy: e.hierarchy.clone().unwrap_or_default(),
            })
            .or_else(|_| {
                resolve_measure(col, snapshot).map(|_| ResolvedKind::Measure)
            })
            .map_err(|_| {
                MqoFromSqlError::Resolve(ResolveError::UnknownName(col.clone()))
            })?;

        let filter = match clause.op.as_str() {
            "=" | "IN" => {
                // Build a Member filter
                let members = match &clause.value {
                    serde_json::Value::Array(arr) => arr
                        .iter()
                        .map(json_val_to_string)
                        .collect::<Vec<_>>(),
                    v => vec![json_val_to_string(v)],
                };

                let hierarchy = match &resolved_kind {
                    ResolvedKind::DimensionLevel { hierarchy } => hierarchy.clone(),
                    ResolvedKind::Measure => col.clone(),
                };

                Filter::Member { hierarchy, members }
            }
            "<" | ">" | "<=" | ">=" | "!=" => {
                // Build a Range filter for numeric comparisons
                let num = json_val_to_f64(&clause.value).ok_or_else(|| {
                    MqoFromSqlError::Resolve(ResolveError::UnknownName(format!(
                        "non-numeric value in range filter: {}",
                        clause.value
                    )))
                })?;

                use mqo_spec::RangeBound;
                let (lo, hi) = match clause.op.as_str() {
                    "<" => (RangeBound::Number(f64::NEG_INFINITY), RangeBound::Number(num - 1.0)),
                    ">" => (RangeBound::Number(num + 1.0), RangeBound::Number(f64::INFINITY)),
                    "<=" => (RangeBound::Number(f64::NEG_INFINITY), RangeBound::Number(num)),
                    ">=" => (RangeBound::Number(num), RangeBound::Number(f64::INFINITY)),
                    _ => (RangeBound::Number(num), RangeBound::Number(num)),
                };

                Filter::Range {
                    level: col.clone(),
                    lo,
                    hi,
                }
            }
            op => {
                return Err(MqoFromSqlError::Parse(
                    crate::error::ParseError::UnsupportedShape(format!(
                        "unsupported filter operator: {op}"
                    )),
                ))
            }
        };

        filters.push(filter);
    }

    Ok(filters)
}

fn json_val_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn json_val_to_f64(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}
