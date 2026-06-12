//! AC3: The slice op filters to California only; input handle is unchanged
//! (immutable-derive: original rows still present).

use mqo_duckdb_handle_store::{MemStore, ResultStore};
use mqo_handle_walkthrough::ops;

#[path = "helpers.rs"]
mod helpers;

#[test]
fn ac3_slice_returns_only_california_rows() {
    let rows = helpers::load_fixture();
    let ca_rows = ops::slice_by_state(&rows, "California");
    assert!(
        !ca_rows.is_empty(),
        "should have at least one California row"
    );
    for r in &ca_rows {
        let state = r.get("state").and_then(|v| v.as_str()).unwrap_or("");
        assert_eq!(state, "California", "all sliced rows must be California");
    }
}

#[test]
fn ac3_input_handle_unchanged_after_slice() {
    let rows = helpers::load_fixture();
    let now_unix: u64 = 1_717_920_000;
    let schema = ops::infer_schema(&rows);

    let mut store = MemStore::with_defaults();
    let env_b = store.put(&rows, &schema, now_unix).unwrap();

    // Derive slice.
    let rows_b = store.get_rows(&env_b.handle, 0, usize::MAX).unwrap();
    let ca_rows = ops::slice_by_state(&rows_b, "California");
    let schema_c = ops::infer_schema(&ca_rows);
    let _env_c = store.put(&ca_rows, &schema_c, now_unix).unwrap();

    // Original handle_B still returns all rows.
    let rows_b_again = store.get_rows(&env_b.handle, 0, usize::MAX).unwrap();
    assert_eq!(
        rows_b_again.len(),
        rows.len(),
        "input handle must be unchanged after slice"
    );
    // And the new handle_C has only California rows.
    assert!(
        ca_rows.len() < rows.len(),
        "slice must produce fewer rows than input"
    );
    for r in &ca_rows {
        assert_eq!(
            r.get("state").and_then(|v| v.as_str()),
            Some("California")
        );
    }
}
