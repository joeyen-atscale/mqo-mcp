/// AC5: open() folds the log to latest-state — after N appends touching M
/// distinct findings, all() returns exactly M findings with the latest
/// status/recurrence each.
use mcp_finding_store::{FindingStatus, FindingStore};

#[test]
fn ac5_fold() {
    let dir = tempfile::tempdir().unwrap();
    let store = FindingStore::open(dir.path()).unwrap();

    let watch = serde_json::json!({});
    let resolved = serde_json::json!({});

    // Create 3 distinct findings
    let fid_a = store
        .record("qid-a", &watch, &resolved, FindingStatus::Open, 1000)
        .unwrap();
    let fid_b = store
        .record("qid-b", &watch, &resolved, FindingStatus::Open, 2000)
        .unwrap();
    let fid_c = store
        .record("qid-c", &watch, &resolved, FindingStatus::Open, 3000)
        .unwrap();

    // Supersede A (recurrence_count becomes 1)
    store
        .record("qid-a", &watch, &resolved, FindingStatus::Open, 4000)
        .unwrap();

    // Update B status to Confirmed
    store
        .set_status(&fid_b, FindingStatus::Confirmed, 5000)
        .unwrap();

    // Update C status to Refuted
    store
        .set_status(&fid_c, FindingStatus::Refuted, 6000)
        .unwrap();

    // all() should return exactly 3 findings (M=3 distinct finding_ids)
    let all = store.all().unwrap();
    assert_eq!(all.len(), 3, "fold must produce exactly M=3 findings");

    // Verify folded state for A
    let a = all.iter().find(|f| f.finding_id == fid_a).unwrap();
    assert_eq!(a.recurrence_count, 1, "A should have recurrence_count=1");
    assert_eq!(a.last_seen_ms, 4000);
    assert_eq!(a.status, FindingStatus::Open);

    // Verify folded state for B
    let b = all.iter().find(|f| f.finding_id == fid_b).unwrap();
    assert_eq!(b.status, FindingStatus::Confirmed, "B should be Confirmed");
    assert_eq!(b.recurrence_count, 0);

    // Verify folded state for C
    let c = all.iter().find(|f| f.finding_id == fid_c).unwrap();
    assert_eq!(c.status, FindingStatus::Refuted, "C should be Refuted");
}
