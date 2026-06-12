/// AC6: A hand-corrupted line causes open()/all() to Err naming the line
/// number; no silent skip.
use mcp_finding_store::FindingStore;
use std::io::Write;

#[test]
fn ac6_corrupt() {
    let dir = tempfile::tempdir().unwrap();

    // Write a valid line, a corrupt line, and another valid line manually
    let path = dir.path().join("findings.jsonl");
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&path)
            .unwrap();

        // Valid new record
        writeln!(
            f,
            r#"{{"record_type":"new","finding_id":"qid-x-aaa","query_id":"qid-x","watch_event":{{}},"resolved_hypotheses":{{}},"status":"Open","recurrence_count":0,"first_seen_ms":1000,"last_seen_ms":1000}}"#
        ).unwrap();

        // Corrupt line (not valid JSON)
        writeln!(f, "THIS IS NOT JSON {{{{ corrupt %%%%").unwrap();

        // Another valid record
        writeln!(
            f,
            r#"{{"record_type":"new","finding_id":"qid-y-bbb","query_id":"qid-y","watch_event":{{}},"resolved_hypotheses":{{}},"status":"Open","recurrence_count":0,"first_seen_ms":2000,"last_seen_ms":2000}}"#
        ).unwrap();
    }

    let store = FindingStore::open(dir.path()).unwrap();
    let result = store.all();

    assert!(result.is_err(), "all() must return Err on corrupt line");
    let err = result.unwrap_err();
    let msg = err.to_string();
    // The error message must mention the line number (2)
    assert!(
        msg.contains("2"),
        "error message should mention line number 2, got: {}",
        msg
    );
    // Should also say "corrupt"
    assert!(
        msg.to_lowercase().contains("corrupt"),
        "error message should mention 'corrupt', got: {}",
        msg
    );
}
