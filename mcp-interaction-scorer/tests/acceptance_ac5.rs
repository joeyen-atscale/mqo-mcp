//! AC5 (SHOULD): empty JSONL file returns Ok with empty maps.

use mcp_interaction_scorer::score_reader;

#[test]
fn ac5_empty_file_returns_ok_empty_maps() {
    let empty: &[u8] = b"";
    let report = score_reader(std::io::Cursor::new(empty))
        .expect("empty JSONL should return Ok, not Err");
    assert!(
        report.sessions.is_empty(),
        "sessions map should be empty for empty input"
    );
    assert!(
        report.entities.is_empty(),
        "entities map should be empty for empty input"
    );
}
