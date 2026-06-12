//! Core walkthrough engine: runs the 4-turn script against a `ResultStore`.

use mqo_duckdb_handle_store::{ColumnSchema, DatasetHandle, ResultStore};
use serde_json::Value;

use crate::ops;
use crate::transcript::{TurnRecord, WalkthroughHeader, WalkthroughTranscript};

/// Result of executing the walkthrough.
#[derive(Debug)]
pub struct WalkthroughResult {
    pub transcript: WalkthroughTranscript,
    /// The Vega-Lite spec from turn 4.
    pub vega_lite: Value,
}

/// Run the default 4-turn walkthrough.
///
/// `seed_rows`: the rows from turn 1 (either loaded from fixture or from live query).
/// `requery_count`: must be 1 at the end (caller passes in the counter value after
///                  the single AtScale round-trip that produced `seed_rows`).
/// `store`: the handle store (mem or duckdb).
/// `backend_name`: "mem" or "duckdb" for the transcript header.
pub fn run_default_script<S: ResultStore>(
    seed_rows: Vec<Value>,
    requery_count: usize,
    store: &mut S,
    backend_name: &str,
) -> Result<WalkthroughResult, String> {
    let now_unix: u64 = 1_717_920_000; // fixed epoch for determinism

    let mut turns: Vec<TurnRecord> = Vec::new();

    // ── Turn 1: store seed rows → handle_A ─────────────────────────────────
    let schema_a = ops::infer_schema(&seed_rows);
    let env_a = store
        .put(&seed_rows, &schema_a, now_unix)
        .map_err(|e| format!("turn1 put failed: {e}"))?;
    let handle_a: DatasetHandle = env_a.handle.clone();
    turns.push(TurnRecord {
        turn: 1,
        op: "query".to_string(),
        input_handle: None,
        output_handle: handle_a.to_string(),
        row_count: env_a.row_count,
        vega_lite_spec: None,
    });

    // ── Turn 2: period_over_period over handle_A → handle_B ────────────────
    let rows_a = store
        .get_rows(&handle_a, 0, usize::MAX)
        .map_err(|e| format!("turn2 get_rows failed: {e}"))?;
    let rows_b = ops::period_over_period(&rows_a);
    let schema_b = ops::infer_schema(&rows_b);
    let env_b = store
        .put(&rows_b, &schema_b, now_unix)
        .map_err(|e| format!("turn2 put failed: {e}"))?;
    let handle_b: DatasetHandle = env_b.handle.clone();
    turns.push(TurnRecord {
        turn: 2,
        op: "period_over_period".to_string(),
        input_handle: Some(handle_a.to_string()),
        output_handle: handle_b.to_string(),
        row_count: env_b.row_count,
        vega_lite_spec: None,
    });

    // ── Turn 3: slice handle_B → California → handle_C ─────────────────────
    let rows_b2 = store
        .get_rows(&handle_b, 0, usize::MAX)
        .map_err(|e| format!("turn3 get_rows failed: {e}"))?;
    let rows_c = ops::slice_by_state(&rows_b2, "California");
    let schema_c = ops::infer_schema(&rows_c);
    let env_c = store
        .put(&rows_c, &schema_c, now_unix)
        .map_err(|e| format!("turn3 put failed: {e}"))?;
    let handle_c: DatasetHandle = env_c.handle.clone();
    turns.push(TurnRecord {
        turn: 3,
        op: "slice".to_string(),
        input_handle: Some(handle_b.to_string()),
        output_handle: handle_c.to_string(),
        row_count: env_c.row_count,
        vega_lite_spec: None,
    });

    // ── Turn 4: chart handle_C → Vega-Lite spec ────────────────────────────
    let rows_c2 = store
        .get_rows(&handle_c, 0, usize::MAX)
        .map_err(|e| format!("turn4 get_rows failed: {e}"))?;
    let chart_spec = ops::chart_vega_lite(
        &rows_c2,
        "California Web Sales — Monthly Trend",
    );
    // chart handle: store the spec JSON as a single-row "result"
    let chart_row = serde_json::json!({"vega_lite_spec": chart_spec.clone()});
    let schema_chart = vec![ColumnSchema {
        name: "vega_lite_spec".to_string(),
        ty: "json".to_string(),
    }];
    let env_chart = store
        .put(&[chart_row], &schema_chart, now_unix)
        .map_err(|e| format!("turn4 put failed: {e}"))?;
    turns.push(TurnRecord {
        turn: 4,
        op: "chart".to_string(),
        input_handle: Some(handle_c.to_string()),
        output_handle: env_chart.handle.to_string(),
        row_count: rows_c2.len(),
        vega_lite_spec: Some(chart_spec.clone()),
    });

    // ── Re-query guard ──────────────────────────────────────────────────────
    if requery_count != 1 {
        return Err(format!(
            "ASSERTION FAILED: requery_count must be exactly 1, got {requery_count}"
        ));
    }

    let total_handles = turns.len(); // 4 handles allocated
    let transcript = WalkthroughTranscript {
        header: WalkthroughHeader {
            requery_count,
            store_backend: backend_name.to_string(),
            total_handles,
        },
        turns,
    };

    Ok(WalkthroughResult {
        transcript,
        vega_lite: chart_spec,
    })
}
