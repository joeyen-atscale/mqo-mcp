//! AC6: Cross-backend parity — mem and duckdb produce identical op results.
//!
//! The mem path runs in default CI; the duckdb path is feature-gated.

use mqo_duckdb_handle_store::MemStore;
use mqo_handle_walkthrough::walkthrough::run_default_script;

#[path = "helpers.rs"]
mod helpers;

#[test]
fn ac6_mem_backend_runs_offline() {
    let rows = helpers::load_fixture();
    let mut store = MemStore::with_defaults();
    let result = run_default_script(rows, 1, &mut store, "mem").expect("mem run must succeed");
    assert_eq!(result.transcript.header.store_backend, "mem");
    assert_eq!(result.transcript.turns.len(), 4);
}

#[cfg(feature = "duckdb")]
#[test]
fn ac6_duckdb_backend_parity_with_mem() {
    use mqo_duckdb_handle_store::DuckStore;

    let rows = helpers::load_fixture();
    let mut mem_store = MemStore::with_defaults();
    let mem_result =
        run_default_script(rows.clone(), 1, &mut mem_store, "mem").expect("mem run");

    let mut duck_store = DuckStore::open_in_memory().expect("duckdb store");
    let duck_result =
        run_default_script(rows, 1, &mut duck_store, "duckdb").expect("duckdb run");

    // Both produce 4 turns.
    assert_eq!(mem_result.transcript.turns.len(), 4);
    assert_eq!(duck_result.transcript.turns.len(), 4);

    // Turn 3 (slice) row counts must match.
    let mem_slice_rows = mem_result.transcript.turns[2].row_count;
    let duck_slice_rows = duck_result.transcript.turns[2].row_count;
    assert_eq!(
        mem_slice_rows, duck_slice_rows,
        "slice row counts must match across backends"
    );

    // requery_count must be 1 in both.
    assert_eq!(mem_result.transcript.header.requery_count, 1);
    assert_eq!(duck_result.transcript.header.requery_count, 1);
}
