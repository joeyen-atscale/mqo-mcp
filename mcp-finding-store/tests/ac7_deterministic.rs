/// AC7: All methods are pure w.r.t. time (caller-supplied now_ms) —
/// identical inputs yield identical stored state across runs.
use mcp_finding_store::{FindingStatus, FindingStore};

fn build_store(dir: &std::path::Path) -> String {
    let store = FindingStore::open(dir).unwrap();
    let watch = serde_json::json!({"metric": "latency", "value": 500});
    let resolved = serde_json::json!({"root_cause": "slow query"});

    // Use hardcoded now_ms values — no system clock
    let fid = store
        .record("qid-lat", &watch, &resolved, FindingStatus::Open, 100_000)
        .unwrap();
    store
        .record("qid-lat", &watch, &resolved, FindingStatus::Open, 200_000)
        .unwrap();
    store
        .set_status(&fid, FindingStatus::Confirmed, 300_000)
        .unwrap();
    fid
}

#[test]
fn ac7_deterministic() {
    let dir1 = tempfile::tempdir().unwrap();
    let dir2 = tempfile::tempdir().unwrap();

    let fid1 = build_store(dir1.path());
    let fid2 = build_store(dir2.path());

    // finding_ids will differ (UUID), but the structural state should be identical
    let store1 = FindingStore::open(dir1.path()).unwrap();
    let store2 = FindingStore::open(dir2.path()).unwrap();

    let f1 = store1.get(&fid1).unwrap().expect("must exist");
    let f2 = store2.get(&fid2).unwrap().expect("must exist");

    // Structural equality (excluding finding_id which contains UUID)
    assert_eq!(f1.query_id, f2.query_id);
    assert_eq!(f1.recurrence_count, f2.recurrence_count);
    assert_eq!(f1.first_seen_ms, f2.first_seen_ms);
    assert_eq!(f1.last_seen_ms, f2.last_seen_ms);
    assert_eq!(f1.status, f2.status);
    assert_eq!(f1.watch_event, f2.watch_event);
    assert_eq!(f1.resolved_hypotheses, f2.resolved_hypotheses);

    // Verify specific values for determinism
    assert_eq!(f1.recurrence_count, 1);
    assert_eq!(f1.first_seen_ms, 100_000);
    // set_status at 300_000 updates last_seen_ms to 300_000
    assert_eq!(f1.last_seen_ms, 300_000);
    assert_eq!(f1.status, FindingStatus::Confirmed);
}
