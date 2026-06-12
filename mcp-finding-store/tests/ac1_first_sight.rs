/// AC1: `record` for a brand-new query_id creates a Finding with
/// recurrence_count == 0 and first_seen_ms == last_seen_ms == now_ms;
/// `get` round-trips it.
use mcp_finding_store::{FindingStatus, FindingStore};

#[test]
fn ac1_first_sight() {
    let dir = tempfile::tempdir().unwrap();
    let store = FindingStore::open(dir.path()).unwrap();

    let watch = serde_json::json!({"query": "cpu_high", "value": 99});
    let resolved = serde_json::json!({"hypothesis": "runaway process", "confirmed": true});
    let now_ms: u64 = 1_000_000;

    let fid = store
        .record("qid-a", &watch, &resolved, FindingStatus::Open, now_ms)
        .unwrap();

    let finding = store.get(&fid).unwrap().expect("finding should exist");

    assert_eq!(finding.query_id, "qid-a");
    assert_eq!(finding.recurrence_count, 0);
    assert_eq!(finding.first_seen_ms, now_ms);
    assert_eq!(finding.last_seen_ms, now_ms);
    assert_eq!(finding.status, FindingStatus::Open);
    assert_eq!(finding.watch_event, watch);
    assert_eq!(finding.resolved_hypotheses, resolved);
}
