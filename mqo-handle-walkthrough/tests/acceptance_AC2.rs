//! AC2: Turns 2–4 never increment the re-query counter; a bugged requery_count
//! of 2 causes the walkthrough to fail loudly.

use mqo_duckdb_handle_store::MemStore;
use mqo_handle_walkthrough::walkthrough::run_default_script;

#[path = "helpers.rs"]
mod helpers;

#[test]
fn ac2_turns_234_do_not_requery() {
    let rows = helpers::load_fixture();
    let mut store = MemStore::with_defaults();
    // requery_count == 1 → success
    let result = run_default_script(rows, 1, &mut store, "mem").expect("must succeed with count=1");
    // Turns 2-4 have input handles (they consumed prior handles, not AtScale).
    for t in result.transcript.turns.iter().skip(1) {
        assert!(t.input_handle.is_some());
    }
}

#[test]
fn ac2_bugged_requery_count_fails_loudly() {
    let rows = helpers::load_fixture();
    let mut store = MemStore::with_defaults();
    // Simulate a bug: requery_count == 2
    let err = run_default_script(rows, 2, &mut store, "mem")
        .expect_err("must fail when requery_count > 1");
    assert!(
        err.contains("ASSERTION FAILED"),
        "error message must indicate assertion failure: {err}"
    );
    assert!(
        err.contains("2"),
        "error must mention the count: {err}"
    );
}
