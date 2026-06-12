//! [`ResultComparator`] implementations: stub and real DAX comparator.
//!
//! This module provides:
//!
//! - [`DaxComparator`] — the production comparator backed by
//!   `pbicorr_dax_result_comparator::compare`.  It converts
//!   `serde_json::Value` rows to a [`pbicorr_dax_result_comparator::ResultValue::Table`]
//!   and delegates comparison to the typed DAX-semantic engine.
//!
//! - [`StubComparator`] — a lightweight local comparator used by unit tests
//!   that do not need the full DAX-semantic ruleset.  It implements the same
//!   [`ResultComparator`] trait and can be swapped in anywhere `DaxComparator`
//!   is used.

use std::collections::BTreeMap;

use pbicorr_dax_result_comparator::{
    CellValue, ResultValue, RowKey, TableResult, TolerancePolicy, Verdict,
};
use serde_json::Value;

use crate::{PairVerdict, ResultComparator};

// ── DaxComparator ──────────────────────────────────────────────────────────

/// Production comparator backed by `pbicorr_dax_result_comparator::compare`.
///
/// JSON rows (`Vec<serde_json::Value>`) are converted to a
/// [`ResultValue::Table`] and compared under a [`TolerancePolicy`].
///
/// Each JSON object row contributes one table row.  Non-object rows (scalar
/// arrays) are treated as single-column rows with the column name `"value"`.
/// `null` cells become [`CellValue::Blank`]; numbers become
/// [`CellValue::Number`]; booleans become [`CellValue::Boolean`]; everything
/// else (string, array, nested object) becomes [`CellValue::Text`] via its
/// JSON representation.
///
/// The row key is constructed by joining all column names and their cell
/// values in sorted column order: `"col1=v1,col2=v2"`.  This makes the
/// comparison order-insensitive by default (matching the default
/// [`TolerancePolicy::order_sensitive`] = `false`).
#[derive(Debug, Clone, Default)]
pub struct DaxComparator {
    policy: TolerancePolicy,
}

impl DaxComparator {
    /// Create a `DaxComparator` with a custom [`TolerancePolicy`].
    #[must_use]
    pub fn with_policy(policy: TolerancePolicy) -> Self {
        Self { policy }
    }
}

impl ResultComparator for DaxComparator {
    fn compare_rows(&self, actual: &[Value], expected: &[Value]) -> PairVerdict {
        let actual_rv = rows_to_result_value(actual);
        let expected_rv = rows_to_result_value(expected);
        verdict_to_pair_verdict(pbicorr_dax_result_comparator::compare(
            &actual_rv,
            &expected_rv,
            &self.policy,
        ))
    }
}

/// Convert a JSON scalar `Value` to the most precise [`CellValue`].
fn json_to_cell(v: &Value) -> CellValue {
    match v {
        Value::Null => CellValue::Blank,
        Value::Bool(b) => CellValue::Boolean(*b),
        Value::Number(n) => CellValue::Number(n.as_f64().unwrap_or(f64::NAN)),
        // Strings, arrays, objects → text representation.
        other => CellValue::Text(other.to_string()),
    }
}

/// Build a stable row key from a JSON object's fields in sorted key order.
fn make_row_key(obj: &serde_json::Map<String, Value>) -> RowKey {
    let mut parts: Vec<String> = obj
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    parts.sort_unstable();
    RowKey(parts.join(","))
}

/// Convert `&[serde_json::Value]` rows to a [`ResultValue`] suitable for the
/// DAX comparator.
///
/// - An empty slice → `ResultValue::Table` with zero rows.
/// - Object rows → keyed by their field values; columns from the union of all
///   field names.
/// - Scalar rows → single-column `"value"` table.
fn rows_to_result_value(rows: &[Value]) -> ResultValue {
    let mut columns: Vec<String> = Vec::new();
    let mut table_rows: BTreeMap<RowKey, BTreeMap<String, CellValue>> = BTreeMap::new();

    for (idx, row) in rows.iter().enumerate() {
        match row {
            Value::Object(obj) => {
                // Collect column names.
                for k in obj.keys() {
                    if !columns.contains(k) {
                        columns.push(k.clone());
                    }
                }
                let key = make_row_key(obj);
                let cells: BTreeMap<String, CellValue> = obj
                    .iter()
                    .map(|(k, v)| (k.clone(), json_to_cell(v)))
                    .collect();
                table_rows.insert(key, cells);
            }
            scalar => {
                // Non-object: treat as single-column "value" table.
                if !columns.contains(&"value".to_owned()) {
                    columns.push("value".to_owned());
                }
                let key = RowKey(format!("row={idx}"));
                let mut cells = BTreeMap::new();
                cells.insert("value".to_owned(), json_to_cell(scalar));
                table_rows.insert(key, cells);
            }
        }
    }

    columns.sort_unstable();
    ResultValue::Table(TableResult {
        columns,
        rows: table_rows,
    })
}

/// Map a [`Verdict`] from the DAX comparator to our crate's [`PairVerdict`].
fn verdict_to_pair_verdict(v: Verdict) -> PairVerdict {
    match v {
        Verdict::Equal => PairVerdict::Equal,
        Verdict::WithinTolerance { detail } => PairVerdict::WithinTolerance { detail },
        Verdict::Mismatch { reason } => PairVerdict::Mismatch {
            reason: reason.to_string(),
        },
    }
}

/// Default numeric tolerance: 0.01% relative error.
const DEFAULT_TOLERANCE_PCT: f64 = 0.0001;

/// Stub comparator.
///
/// Implements the following rules (a subset of DAX semantics):
///
/// - **Row count**: if `actual.len() != expected.len()` → `Mismatch` with a
///   `RowCountDiffers` description.
/// - **Order-insensitive**: rows are sorted by their canonical JSON string
///   representation before comparison.
/// - **Numeric tolerance**: numeric scalars are compared within `tolerance_pct`
///   relative error (default [`DEFAULT_TOLERANCE_PCT`]).
/// - **Null/None**: both null → `Equal`; one null one non-null → `Mismatch`.
/// - **Strings**: case-insensitive exact match.
/// - **Booleans**: exact match.
///
/// # Note
///
/// This is intentionally minimal. It exists only to make the architecture
/// compile and tests pass until the real comparator is wired in.
#[derive(Debug, Clone)]
pub struct StubComparator {
    tolerance_pct: f64,
}

impl Default for StubComparator {
    fn default() -> Self {
        Self {
            tolerance_pct: DEFAULT_TOLERANCE_PCT,
        }
    }
}

impl StubComparator {
    /// Create a `StubComparator` with a custom tolerance percentage (e.g. `0.01` = 1%).
    pub fn with_tolerance(tolerance_pct: f64) -> Self {
        Self { tolerance_pct }
    }
}

impl ResultComparator for StubComparator {
    fn compare_rows(
        &self,
        actual: &[Value],
        expected: &[Value],
    ) -> PairVerdict {
        // TODO: replace with pbicorr-dax-result-comparator::compare()
        compare_rows_stub(actual, expected, self.tolerance_pct)
    }
}

// ── Internal helpers ───────────────────────────────────────────────────────

/// Canonical string form of a JSON value for order-insensitive row sorting.
fn canonical(v: &Value) -> String {
    v.to_string()
}

/// Compare two JSON scalar values under tolerance / null / string rules.
///
/// Returns `(equal, within_tolerance, detail)`:
/// - `equal = true` → values are identical
/// - `within_tolerance = true` → values are within numeric tolerance (but not equal)
/// - Otherwise → mismatch
fn compare_values(
    a: &Value,
    b: &Value,
    tolerance_pct: f64,
) -> ValueCmp {
    match (a, b) {
        (Value::Null, Value::Null) => ValueCmp::Equal,

        // One null, one non-null → mismatch.
        (Value::Null, _) | (_, Value::Null) => {
            ValueCmp::Mismatch(format!("null vs non-null: {a} vs {b}"))
        }

        (Value::Bool(x), Value::Bool(y)) => {
            if x == y {
                ValueCmp::Equal
            } else {
                ValueCmp::Mismatch(format!("boolean differs: {x} vs {y}"))
            }
        }

        // Numeric: compare within tolerance.
        (Value::Number(x), Value::Number(y)) => {
            let xf = x.as_f64().unwrap_or(f64::NAN);
            let yf = y.as_f64().unwrap_or(f64::NAN);
            compare_numerics(xf, yf, tolerance_pct)
        }

        // String: case-insensitive.
        (Value::String(x), Value::String(y)) => {
            if x.to_lowercase() == y.to_lowercase() {
                ValueCmp::Equal
            } else {
                ValueCmp::Mismatch(format!("string differs: {x:?} vs {y:?}"))
            }
        }

        // Arrays / Objects: recursive by canonical representation.
        _ => {
            if canonical(a) == canonical(b) {
                ValueCmp::Equal
            } else {
                ValueCmp::Mismatch(format!("value differs: {a} vs {b}"))
            }
        }
    }
}

/// Outcome of comparing two scalar values.
#[derive(Debug)]
enum ValueCmp {
    Equal,
    WithinTolerance(String),
    Mismatch(String),
}

fn compare_numerics(x: f64, y: f64, tolerance_pct: f64) -> ValueCmp {
    // Both NaN → equal (missing/blank).
    if x.is_nan() && y.is_nan() {
        return ValueCmp::Equal;
    }
    if x.is_nan() || y.is_nan() {
        return ValueCmp::Mismatch(format!("one value is NaN: {x} vs {y}"));
    }

    // Exact equality (covers 0.0 == 0.0 without division).
    #[allow(clippy::float_arithmetic)]
    let diff = (x - y).abs();
    if diff == 0.0 {
        return ValueCmp::Equal;
    }

    // Relative tolerance.
    let denom = x.abs().max(y.abs());
    #[allow(clippy::float_arithmetic)]
    let rel = if denom == 0.0 { diff } else { diff / denom };

    if rel <= tolerance_pct {
        ValueCmp::WithinTolerance(format!(
            "numeric within tolerance: {x} vs {y} (rel={rel:.2e}, tol={tolerance_pct:.2e})"
        ))
    } else {
        ValueCmp::Mismatch(format!(
            "numeric beyond tolerance: {x} vs {y} (rel={rel:.2e}, tol={tolerance_pct:.2e})"
        ))
    }
}

/// Core stub comparison logic.
fn compare_rows_stub(actual: &[Value], expected: &[Value], tolerance_pct: f64) -> PairVerdict {
    // Row count check.
    if actual.len() != expected.len() {
        return PairVerdict::Mismatch {
            reason: format!(
                "RowCountDiffers: actual={}, expected={}",
                actual.len(),
                expected.len()
            ),
        };
    }

    if actual.is_empty() {
        return PairVerdict::Equal;
    }

    // Sort rows by canonical string for order-insensitive comparison.
    let mut sorted_actual: Vec<&Value> = actual.iter().collect();
    let mut sorted_expected: Vec<&Value> = expected.iter().collect();
    sorted_actual.sort_by_key(|a| canonical(a));
    sorted_expected.sort_by_key(|a| canonical(a));

    let mut any_tolerance = false;
    let mut tolerance_details: Vec<String> = Vec::new();

    for (row_a, row_b) in sorted_actual.iter().zip(sorted_expected.iter()) {
        match compare_row_pair(row_a, row_b, tolerance_pct) {
            ValueCmp::Equal => {}
            ValueCmp::WithinTolerance(detail) => {
                any_tolerance = true;
                tolerance_details.push(detail);
            }
            ValueCmp::Mismatch(reason) => {
                return PairVerdict::Mismatch { reason };
            }
        }
    }

    if any_tolerance {
        PairVerdict::WithinTolerance {
            detail: tolerance_details.join("; "),
        }
    } else {
        PairVerdict::Equal
    }
}

/// Compare two individual rows (JSON objects or scalars).
fn compare_row_pair(a: &Value, b: &Value, tolerance_pct: f64) -> ValueCmp {
    match (a, b) {
        (Value::Object(map_a), Value::Object(map_b)) => {
            // Compare all fields from both maps.
            let mut all_keys: Vec<&str> = map_a.keys().map(String::as_str).collect();
            for k in map_b.keys() {
                if !all_keys.contains(&k.as_str()) {
                    all_keys.push(k);
                }
            }
            all_keys.sort_unstable();

            let mut any_tolerance = false;
            let mut details = Vec::new();

            for key in all_keys {
                let val_a = map_a.get(key).unwrap_or(&Value::Null);
                let val_b = map_b.get(key).unwrap_or(&Value::Null);
                match compare_values(val_a, val_b, tolerance_pct) {
                    ValueCmp::Equal => {}
                    ValueCmp::WithinTolerance(d) => {
                        any_tolerance = true;
                        details.push(format!("{key}: {d}"));
                    }
                    ValueCmp::Mismatch(reason) => {
                        return ValueCmp::Mismatch(format!("field {key:?}: {reason}"));
                    }
                }
            }

            if any_tolerance {
                ValueCmp::WithinTolerance(details.join("; "))
            } else {
                ValueCmp::Equal
            }
        }
        // Non-object rows: compare as scalars.
        _ => compare_values(a, b, tolerance_pct),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn identical_rows_equal() {
        let cmp = StubComparator::default();
        let rows = vec![json!({"region": "US", "sales": 100})];
        assert_eq!(cmp.compare_rows(&rows, &rows), PairVerdict::Equal);
    }

    #[test]
    fn empty_rows_equal() {
        let cmp = StubComparator::default();
        assert_eq!(cmp.compare_rows(&[], &[]), PairVerdict::Equal);
    }

    #[test]
    fn row_count_mismatch() {
        let cmp = StubComparator::default();
        let a = vec![json!({"v": 1}), json!({"v": 2})];
        let b = vec![json!({"v": 1})];
        assert!(matches!(
            cmp.compare_rows(&a, &b),
            PairVerdict::Mismatch { .. }
        ));
    }

    #[test]
    fn numeric_within_tolerance() {
        let cmp = StubComparator::default();
        // 100.000001 vs 100.0: relative diff ≈ 1e-8, well within 0.01%.
        let a = vec![json!({"v": 100.000_001_f64})];
        let b = vec![json!({"v": 100.0_f64})];
        assert!(
            matches!(cmp.compare_rows(&a, &b), PairVerdict::WithinTolerance { .. }),
            "expected WithinTolerance"
        );
    }

    #[test]
    fn numeric_beyond_tolerance() {
        let cmp = StubComparator::default();
        // 100.0 vs 101.0: relative diff = 1%, beyond 0.01%.
        let a = vec![json!({"v": 100.0_f64})];
        let b = vec![json!({"v": 101.0_f64})];
        assert!(matches!(
            cmp.compare_rows(&a, &b),
            PairVerdict::Mismatch { .. }
        ));
    }

    #[test]
    fn null_vs_null_equal() {
        let cmp = StubComparator::default();
        let a = vec![json!({"v": null})];
        let b = vec![json!({"v": null})];
        assert_eq!(cmp.compare_rows(&a, &b), PairVerdict::Equal);
    }

    #[test]
    fn null_vs_nonnull_mismatch() {
        let cmp = StubComparator::default();
        let a = vec![json!({"v": null})];
        let b = vec![json!({"v": 0})];
        assert!(matches!(
            cmp.compare_rows(&a, &b),
            PairVerdict::Mismatch { .. }
        ));
    }

    #[test]
    fn string_case_insensitive_equal() {
        let cmp = StubComparator::default();
        let a = vec![json!({"region": "US"})];
        let b = vec![json!({"region": "us"})];
        assert_eq!(cmp.compare_rows(&a, &b), PairVerdict::Equal);
    }

    #[test]
    fn order_insensitive_equal() {
        let cmp = StubComparator::default();
        let a = vec![json!({"v": 1}), json!({"v": 2})];
        let b = vec![json!({"v": 2}), json!({"v": 1})];
        assert_eq!(cmp.compare_rows(&a, &b), PairVerdict::Equal);
    }
}
