/// AC4: findings.jsonl is append-only: a supersede and a status change each
/// add a line without rewriting earlier lines; every line is independently
/// valid JSON.
use mcp_finding_store::{FindingStatus, FindingStore};
use std::fs;
use std::io::{BufRead, BufReader};

#[test]
fn ac4_append_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = FindingStore::open(dir.path()).unwrap();

    let watch = serde_json::json!({"query": "mem_high"});
    let resolved = serde_json::json!({"confirmed": false});

    // 1. new record → 1 line
    let fid = store
        .record("qid-mem", &watch, &resolved, FindingStatus::Open, 1000)
        .unwrap();

    let path = dir.path().join("findings.jsonl");
    let line_count_after_new = line_count(&path);
    assert_eq!(line_count_after_new, 1, "new record appends 1 line");

    // 2. supersede → +1 line (recur record)
    store
        .record("qid-mem", &watch, &resolved, FindingStatus::Open, 2000)
        .unwrap();
    let line_count_after_recur = line_count(&path);
    assert_eq!(line_count_after_recur, 2, "supersede appends 1 more line");

    // 3. status change → +1 line (update record)
    store
        .set_status(&fid, FindingStatus::Confirmed, 3000)
        .unwrap();
    let line_count_after_update = line_count(&path);
    assert_eq!(line_count_after_update, 3, "status change appends 1 more line");

    // 4. Every line must be independently valid JSON
    let file = fs::File::open(&path).unwrap();
    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = line.unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("line {} is not valid JSON: {}", i + 1, e));
        assert!(
            parsed.is_object(),
            "line {} must be a JSON object",
            i + 1
        );
    }
}

fn line_count(path: &std::path::Path) -> usize {
    let content = fs::read_to_string(path).unwrap_or_default();
    content.lines().filter(|l| !l.trim().is_empty()).count()
}
