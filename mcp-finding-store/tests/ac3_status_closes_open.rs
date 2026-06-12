/// AC3: After set_status(id, Suppressed, now), a later record for that
/// query_id starts a NEW finding (recurrence_count == 0) — a suppressed
/// finding is no longer "open" to supersede.
use mcp_finding_store::{FindingStatus, FindingStore};

#[test]
fn ac3_status_closes_open() {
    let dir = tempfile::tempdir().unwrap();
    let store = FindingStore::open(dir.path()).unwrap();

    let watch = serde_json::json!({"query": "disk_full"});
    let resolved = serde_json::json!({});
    let now1: u64 = 1_000_000;

    let fid1 = store
        .record("qid-disk", &watch, &resolved, FindingStatus::Open, now1)
        .unwrap();

    // Suppress the finding
    let now2: u64 = 2_000_000;
    let found = store
        .set_status(&fid1, FindingStatus::Suppressed, now2)
        .unwrap();
    assert!(found, "set_status should find the finding");

    // Now record again for the same query_id — should create a NEW finding
    let now3: u64 = 3_000_000;
    let fid2 = store
        .record("qid-disk", &watch, &resolved, FindingStatus::Open, now3)
        .unwrap();

    assert_ne!(fid1, fid2, "should be a new finding after suppression");

    let new_finding = store.get(&fid2).unwrap().expect("new finding must exist");
    assert_eq!(new_finding.recurrence_count, 0, "new finding starts at 0");
    assert_eq!(new_finding.first_seen_ms, now3);
    assert_eq!(new_finding.status, FindingStatus::Open);
}
