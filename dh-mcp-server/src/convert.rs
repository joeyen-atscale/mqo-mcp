//! Convert the MQO pipeline's row output into a typed [`dh_store::Dataset`].
//!
//! The MQO fixture engine emits `rows: Vec<Value>` where each row is a JSON
//! object keyed by *column unique-name* (one entry per bound dimension level and
//! one per bound measure).  The binder's `bound` output carries the **ordered**
//! `dimensions[]` and `measures[]` with their `unique_name`s — that ordering is
//! the authoritative column order and role assignment:
//!
//! * each bound dimension → a [`ColumnRole::Dimension`] column, dtype
//!   [`DType::Str`] (member captions are strings in the fixture engine);
//! * each bound measure → a [`ColumnRole::Measure`] column, dtype
//!   [`DType::Float`] (numeric aggregates).
//!
//! The short display `name` is the leaf segment of the unique-name (the part
//! after the final `.`, with a trailing `]` and leading `[` trimmed); the full
//! unique-name is preserved for stats keying and lineage.

use dh_spec::{ColumnRole, ColumnSchema, DType};
use dh_store::{ColumnData, Dataset};
use serde_json::Value;

/// A column we intend to build, with its role-driven dtype.
struct ColPlan {
    unique_name: String,
    name: String,
    role: ColumnRole,
    dtype: DType,
}

/// Derive the short, human-facing column name from a fully-qualified unique
/// name: take the segment after the final `.`, then strip MDX-style brackets.
fn leaf_name(unique_name: &str) -> String {
    let leaf = unique_name.rsplit('.').next().unwrap_or(unique_name);
    leaf.trim_start_matches('[').trim_end_matches(']').to_string()
}

/// Pull the ordered unique-names out of a `bound[key]` array of objects.
fn bound_unique_names(bound: &Value, key: &str) -> Vec<String> {
    bound
        .get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("unique_name").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Build the column plan (order + role + dtype) from the bound MQO.
fn plan_columns(bound: &Value) -> Vec<ColPlan> {
    let mut plans = Vec::new();
    for un in bound_unique_names(bound, "dimensions") {
        plans.push(ColPlan {
            name: leaf_name(&un),
            unique_name: un,
            role: ColumnRole::Dimension,
            dtype: DType::Str,
        });
    }
    for un in bound_unique_names(bound, "measures") {
        plans.push(ColPlan {
            name: leaf_name(&un),
            unique_name: un,
            role: ColumnRole::Measure,
            dtype: DType::Float,
        });
    }
    plans
}

/// Convert the pipeline's `(bound, rows)` into a [`Dataset`].
///
/// Column order, names, and roles come from `bound`; the cell values come from
/// `rows` (looked up by unique-name).  Missing cells become typed nulls.
///
/// # Errors
///
/// Returns `Err(String)` when the bound MQO has neither dimensions nor measures
/// (an empty projection cannot form a dataset) or the assembled columns fail the
/// [`Dataset`] alignment invariant (which should not happen — all columns are
/// built to `rows.len()`).
pub fn rows_to_dataset(bound: &Value, rows: &[Value]) -> Result<Dataset, String> {
    let plans = plan_columns(bound);
    if plans.is_empty() {
        return Err("bound MQO projects no columns (no dimensions and no measures)".to_string());
    }

    let mut columns: Vec<ColumnSchema> = Vec::with_capacity(plans.len());
    let mut data: Vec<ColumnData> = Vec::with_capacity(plans.len());

    for plan in &plans {
        columns.push(ColumnSchema {
            name: plan.name.clone(),
            unique_name: plan.unique_name.clone(),
            dtype: plan.dtype,
            nullable: true,
            role: plan.role,
        });

        let col_data = match plan.dtype {
            DType::Float => {
                let vals: Vec<Option<f64>> = rows
                    .iter()
                    .map(|r| r.get(&plan.unique_name).and_then(Value::as_f64))
                    .collect();
                ColumnData::Float(vals)
            }
            // Dimensions (and any non-numeric column) render as strings.
            _ => {
                let vals: Vec<Option<String>> = rows
                    .iter()
                    .map(|r| {
                        r.get(&plan.unique_name).and_then(|v| match v {
                            Value::String(s) => Some(s.clone()),
                            Value::Null => None,
                            other => Some(other.to_string()),
                        })
                    })
                    .collect();
                ColumnData::Str(vals)
            }
        };
        data.push(col_data);
    }

    Dataset::new(columns, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn bound() -> Value {
        json!({
            "dimensions": [{ "unique_name": "time.calendar.[Year]", "hierarchy": "time.calendar" }],
            "measures": [{ "unique_name": "sales.revenue" }],
        })
    }

    #[test]
    fn leaf_name_strips_brackets_and_prefix() {
        assert_eq!(leaf_name("time.calendar.[Year]"), "Year");
        assert_eq!(leaf_name("sales.revenue"), "revenue");
        assert_eq!(leaf_name("bare"), "bare");
    }

    #[test]
    fn builds_typed_columns_in_bound_order() {
        let rows = vec![
            json!({ "time.calendar.[Year]": "Year-0", "sales.revenue": 1000.0 }),
            json!({ "time.calendar.[Year]": "Year-1", "sales.revenue": 1010.0 }),
        ];
        let ds = rows_to_dataset(&bound(), &rows).unwrap();
        assert_eq!(ds.row_count(), 2);
        assert_eq!(ds.columns.len(), 2);
        // dimension first, measure second
        assert_eq!(ds.columns[0].name, "Year");
        assert_eq!(ds.columns[0].role, ColumnRole::Dimension);
        assert_eq!(ds.columns[1].name, "revenue");
        assert_eq!(ds.columns[1].role, ColumnRole::Measure);
        assert!(matches!(ds.data[0], ColumnData::Str(_)));
        assert!(matches!(ds.data[1], ColumnData::Float(_)));
    }

    #[test]
    fn empty_projection_is_rejected() {
        let b = json!({ "dimensions": [], "measures": [] });
        assert!(rows_to_dataset(&b, &[]).is_err());
    }

    #[test]
    fn missing_cells_become_typed_nulls() {
        let rows = vec![json!({ "time.calendar.[Year]": "Year-0" })]; // no revenue key
        let ds = rows_to_dataset(&bound(), &rows).unwrap();
        if let ColumnData::Float(v) = &ds.data[1] {
            assert_eq!(v[0], None);
        } else {
            panic!("measure column should be Float");
        }
    }
}
