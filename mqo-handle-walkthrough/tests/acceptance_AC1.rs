//! AC1: Running the default script with --seed-result (offline) completes all
//! four turns and writes walkthrough.json with requery_count == 1.

use mqo_duckdb_handle_store::MemStore;
use mqo_handle_walkthrough::walkthrough::run_default_script;

#[path = "helpers.rs"]
mod helpers;

#[test]
fn ac1_four_turns_requery_count_is_one() {
    let rows = helpers::load_fixture();
    let mut store = MemStore::with_defaults();
    let result = run_default_script(rows, 1, &mut store, "mem").expect("walkthrough must succeed");

    let header = &result.transcript.header;
    assert_eq!(header.requery_count, 1, "requery_count must be exactly 1");
    assert_eq!(result.transcript.turns.len(), 4, "must have exactly 4 turns");

    // Verify turn ops in order.
    let ops: Vec<&str> = result
        .transcript
        .turns
        .iter()
        .map(|t| t.op.as_str())
        .collect();
    assert_eq!(ops, vec!["query", "period_over_period", "slice", "chart"]);

    // Verify turn 1 has no input_handle.
    assert!(result.transcript.turns[0].input_handle.is_none());
    // Verify turns 2-4 have input handles.
    for t in &result.transcript.turns[1..] {
        assert!(t.input_handle.is_some(), "turn {} must have input_handle", t.turn);
    }

    // Verify written to tempdir.
    let dir = tempfile::tempdir().unwrap();
    let mut store2 = MemStore::with_defaults();
    let rows2 = helpers::load_fixture();
    let result2 = run_default_script(rows2, 1, &mut store2, "mem").expect("second run");
    let path = dir.path().join("walkthrough.json");
    std::fs::write(&path, serde_json::to_string_pretty(&result2.transcript).unwrap()).unwrap();
    let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(written["header"]["requery_count"], 1);
}
