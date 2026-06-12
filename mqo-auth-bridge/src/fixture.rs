//! The fixture engine — deterministic, cluster-free row synthesis.
//!
//! # Design
//!
//! `FixtureEngine` holds an optional JSON `bound` value (the `BoundMqo`
//! output from `mqo-bind`). When `bound` is supplied, row synthesis mirrors
//! `mqo-mcp-server/src/engine.rs` exactly: one column per bound dimension
//! level + one column per bound measure, with deterministic synthetic values.
//! When `bound` is absent, `compiled_query` is used as a placeholder and a
//! single-column synthetic result is returned.
//!
//! This is the simplest design that preserves AC2 determinism:
//! - Scalar (no-dim) bound → exactly 1 row.
//! - Otherwise: `min(limit | DEFAULT_ROWS, HARD_ROW_CAP)` rows.
//!
//! The fixture engine is the server's default path for cluster-free CI.

use serde_json::{Map, Value};

use crate::{
    backend::Backend,
    engine::{Engine, EngineResult, HARD_ROW_CAP},
    error::EngineError,
};

/// Default row count when no limit is specified.
const DEFAULT_ROWS: usize = 5;

/// Deterministic, cluster-free engine.
///
/// Construct with [`FixtureEngine::new`] (no bound, uses `compiled_query`
/// as a placeholder key) or [`FixtureEngine::with_bound`] (full column-name
/// fidelity matching `mqo-mcp-server`'s fixture engine behavior).
#[derive(Debug, Clone, Default)]
pub struct FixtureEngine {
    /// Optional bound MQO JSON — if supplied, used for column names.
    bound: Option<Value>,
}

impl FixtureEngine {
    /// Create a fixture engine with no bound; column names are synthesized
    /// from `compiled_query`.
    #[must_use]
    pub fn new() -> Self {
        Self { bound: None }
    }

    /// Create a fixture engine with a pre-bound `BoundMqo` JSON value.
    ///
    /// This is the path that exactly replicates `mqo-mcp-server/src/engine.rs`.
    #[must_use]
    pub fn with_bound(bound: Value) -> Self {
        Self { bound: Some(bound) }
    }
}

impl Engine for FixtureEngine {
    fn execute(
        &self,
        compiled_query: &str,
        backend: Backend,
        limit: Option<u64>,
        _model: Option<&str>,
    ) -> Result<EngineResult, EngineError> {
        let backend_key = backend.as_fixture_key();

        let (dim_names, measure_names): (Vec<String>, Vec<String>) =
            if let Some(bound) = &self.bound {
                let dims = bound
                    .get("dimensions")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|d| d.get("unique_name").and_then(Value::as_str))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();

                let measures = bound
                    .get("measures")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|m| m.get("unique_name").and_then(Value::as_str))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();

                (dims, measures)
            } else {
                // No bound: synthesize a single measure column named after the
                // compiled query (truncated for readability).
                let key = compiled_query
                    .split_whitespace()
                    .next()
                    .unwrap_or("query")
                    .to_string();
                (vec![], vec![key])
            };

        // How many rows to emit. Scalar (no dims) → always 1 row.
        let want = if dim_names.is_empty() {
            1
        } else {
            let l = limit.map_or(DEFAULT_ROWS, |l| usize::try_from(l).unwrap_or(DEFAULT_ROWS));
            l.clamp(1, HARD_ROW_CAP)
        };

        let mut rows = Vec::with_capacity(want);
        for i in 0..want {
            let mut obj = Map::new();
            for dim in &dim_names {
                let leaf = dim
                    .rsplit(['.', '['])
                    .next()
                    .unwrap_or(dim.as_str())
                    .trim_end_matches(']');
                obj.insert(dim.clone(), Value::String(format!("{leaf}-{i}")));
            }
            for (j, meas) in measure_names.iter().enumerate() {
                let v = synth_measure_value(i, j, backend_key);
                obj.insert(meas.clone(), Value::from(v));
            }
            rows.push(Value::Object(obj));
        }

        Ok(EngineResult::new(rows))
    }
}

/// Deterministic synthetic measure value.
///
/// Backend is folded in so that the three backend paths produce visibly
/// distinct fixture output, exactly as in `mqo-mcp-server/src/engine.rs`.
fn synth_measure_value(row: usize, col: usize, backend: &str) -> f64 {
    let backend_offset = match backend {
        "dax" => 1000.0,
        "mdx" => 2000.0,
        "sql" => 3000.0,
        _ => 0.0,
    };
    let row = u32::try_from(row).unwrap_or(u32::MAX);
    let col = u32::try_from(col).unwrap_or(u32::MAX);
    backend_offset + f64::from(row) * 10.0 + f64::from(col)
}
