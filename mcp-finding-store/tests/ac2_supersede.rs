/// AC2: A second `record` for the same query_id while the first is still
/// open bumps recurrence_count to 1, sets last_seen_ms to the new now_ms,
/// preserves the original first_seen_ms, and does NOT create a second open
/// finding (open_for_query returns one).
use mcp_finding_store::{FindingStatus, FindingStore};

#[test]
fn ac2_supersede() {
    let dir = tempfile::tempdir().unwrap();
    let store = FindingStore::open(dir.path()).unwrap();

    let watch1 = serde_json::json!({"query": "cpu_high", "value": 99});
    let resolved1 = serde_json::json!({"hypothesis": "runaway process"});
    let now1: u64 = 1_000_000;

    let fid1 = store
        .record("qid-a", &watch1, &resolved1, FindingStatus::Open, now1)
        .unwrap();

    let watch2 = serde_json::json!({"query": "cpu_high", "value": 98});
    let resolved2 = serde_json::json!({"hypothesis": "still same process"});
    let now2: u64 = 2_000_000;

    let fid2 = store
        .record("qid-a", &watch2, &resolved2, FindingStatus::Open, now2)
        .unwrap();

    // Same finding_id — not a new one
    assert_eq!(fid1, fid2, "supersede should return the original finding_id");

    let finding = store.get(&fid1).unwrap().expect("finding should exist");
    assert_eq!(finding.recurrence_count, 1);
    assert_eq!(finding.first_seen_ms, now1, "first_seen_ms must be preserved");
    assert_eq!(finding.last_seen_ms, now2, "last_seen_ms should be updated");
    assert_eq!(finding.watch_event, watch2, "watch_event should be replaced");

    // open_for_query returns exactly one finding
    let active = store.open_for_query("qid-a").unwrap();
    assert!(active.is_some(), "should have one active finding");

    // all() should contain exactly one finding for qid-a
    let all = store.all().unwrap();
    let qida: Vec<_> = all.iter().filter(|f| f.query_id == "qid-a").collect();
    assert_eq!(qida.len(), 1, "only one finding should exist for qid-a");
}
