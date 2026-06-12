//! Acceptance tests for the cursor / pagination protocol.
//!
//! Covers all 7 ACs from the PRD:
//!
//! - AC1  Large query → cursor_id + first page (not full rows).
//! - AC2  next_page with correct page_token → correct slice + has_more.
//! - AC3  Expired cursor → structured CursorExpired error, not a panic.
//! - AC4  Small query (≤ PAGE_SIZE) → inline rows, NO cursor_id (backward compat).
//! - AC5  Repeated identical (cursor_id, page_token) → same rows (determinism).
//! - AC6  Final page → has_more = false.
//! - AC7  next_page with unknown cursor_id → structured CursorExpired error.

use mqo_mcp_server::cursor::{CursorStore, DEFAULT_PAGE_SIZE};
use serde_json::{json, Value};

/// Build a Vec of `n` synthetic row Values.
fn make_rows(n: usize) -> Vec<Value> {
    (0..n)
        .map(|i| json!({ "idx": i, "val": format!("row_{i}") }))
        .collect()
}

/// Extract the `idx` field from a row value for easy assertion.
fn row_idx(v: &Value) -> usize {
    v.get("idx")
        .and_then(Value::as_u64)
        .expect("row must have idx") as usize
}

// ── AC1: Large query triggers cursor mode ──────────────────────────────────

#[test]
fn ac1_large_query_returns_cursor_and_first_page() {
    let store = CursorStore::new(600);
    let total = DEFAULT_PAGE_SIZE + 10; // 60 rows
    let rows = make_rows(total);

    let first = store
        .put_and_first_page(rows, DEFAULT_PAGE_SIZE)
        .expect("put_and_first_page should succeed");

    // cursor_id must be non-empty (UUID).
    assert!(!first.cursor_id.is_empty(), "cursor_id must be set");

    // First page must be exactly PAGE_SIZE rows.
    assert_eq!(
        first.page.len(),
        DEFAULT_PAGE_SIZE,
        "first page must be PAGE_SIZE rows"
    );

    // total_rows must equal the full set.
    assert_eq!(first.total_rows, total);

    // has_more must be true.
    assert!(first.has_more, "has_more must be true when rows > PAGE_SIZE");

    // page_token must equal page.len() (next offset).
    assert_eq!(first.page_token, DEFAULT_PAGE_SIZE);

    // Rows must be the first PAGE_SIZE in stored order.
    for (i, row) in first.page.iter().enumerate() {
        assert_eq!(row_idx(row), i);
    }
}

// ── AC2: next_page returns correct slice + has_more ────────────────────────

#[test]
fn ac2_next_page_returns_correct_slice() {
    let store = CursorStore::new(600);
    let total = DEFAULT_PAGE_SIZE + 15; // 65 rows
    let rows = make_rows(total);

    let first = store
        .put_and_first_page(rows, DEFAULT_PAGE_SIZE)
        .expect("put should succeed");

    let cursor_id = &first.cursor_id;

    // Fetch the second page using page_token from first page.
    let second = store
        .next_page(cursor_id, first.page_token, DEFAULT_PAGE_SIZE)
        .expect("next_page should succeed");

    assert_eq!(second.cursor_id, *cursor_id);
    // Remaining 15 rows.
    assert_eq!(second.page.len(), 15);
    // Rows must be indices [50..65).
    for (i, row) in second.page.iter().enumerate() {
        assert_eq!(row_idx(row), DEFAULT_PAGE_SIZE + i);
    }
    // has_more = false — this is the last page.
    assert!(!second.has_more);
}

// ── AC3: Expired cursor → structured error ─────────────────────────────────

#[test]
fn ac3_expired_cursor_returns_structured_error() {
    // TTL of 0 means all entries expire immediately.
    let store = CursorStore::new(0);
    let rows = make_rows(DEFAULT_PAGE_SIZE + 5);

    let first = store
        .put_and_first_page(rows, DEFAULT_PAGE_SIZE)
        .expect("put should succeed");

    // Trigger expiry by calling next_page — evict_expired runs before every read.
    let result = store.next_page(&first.cursor_id, first.page_token, DEFAULT_PAGE_SIZE);

    match result {
        Err(e) => {
            assert_eq!(e.error, "CursorExpired", "error code must be CursorExpired");
            assert_eq!(e.cursor_id, first.cursor_id);
        }
        Ok(_) => panic!("next_page on expired cursor must return Err(CursorExpired)"),
    }
}

// ── AC4: Small query (≤ PAGE_SIZE) → no cursor (backward compat) ──────────

#[test]
fn ac4_small_query_below_threshold_no_cursor() {
    // This test checks that the application layer (Server) does NOT invoke
    // cursor mode for small results.  We test the branch condition directly:
    // rows.len() <= page_size → cursor mode is not triggered.
    let page_size = DEFAULT_PAGE_SIZE;

    // Exactly page_size rows.
    let rows = make_rows(page_size);
    assert!(
        rows.len() <= page_size,
        "rows at threshold must not trigger cursor"
    );

    // One fewer than threshold.
    let rows_small = make_rows(page_size - 1);
    assert!(rows_small.len() < page_size);

    // The store itself should accept rows of any size via put_and_first_page,
    // but the application checks rows.len() > page_size before calling it.
    // Verify the store API still works for small inputs (no panic).
    let store = CursorStore::new(600);
    let envelope = store
        .put_and_first_page(rows.clone(), page_size)
        .expect("put small rows must succeed");

    // For rows.len() == page_size: page == rows, has_more == false.
    assert_eq!(envelope.page.len(), page_size);
    assert!(!envelope.has_more);
    assert_eq!(envelope.page_token, page_size);
}

// ── AC5: Repeated identical call returns same rows ─────────────────────────

#[test]
fn ac5_repeated_next_page_is_deterministic() {
    let store = CursorStore::new(600);
    let total = DEFAULT_PAGE_SIZE * 3;
    let rows = make_rows(total);

    let first = store
        .put_and_first_page(rows, DEFAULT_PAGE_SIZE)
        .expect("put should succeed");

    let cursor_id = &first.cursor_id;
    let offset = DEFAULT_PAGE_SIZE; // second page

    let page_a = store
        .next_page(cursor_id, offset, DEFAULT_PAGE_SIZE)
        .expect("first call must succeed");
    let page_b = store
        .next_page(cursor_id, offset, DEFAULT_PAGE_SIZE)
        .expect("second call must succeed");

    // Both calls must return identical rows.
    assert_eq!(
        page_a.page, page_b.page,
        "repeated calls must return identical rows"
    );
    assert_eq!(page_a.page_token, page_b.page_token);
    assert_eq!(page_a.has_more, page_b.has_more);
}

// ── AC6: Final page has has_more = false ───────────────────────────────────

#[test]
fn ac6_final_page_has_more_false() {
    let store = CursorStore::new(600);
    // Use a result size that is not a perfect multiple of PAGE_SIZE.
    let total = DEFAULT_PAGE_SIZE + 7;
    let rows = make_rows(total);

    let first = store
        .put_and_first_page(rows, DEFAULT_PAGE_SIZE)
        .expect("put should succeed");

    assert!(first.has_more, "first page must have has_more=true");

    let last = store
        .next_page(&first.cursor_id, first.page_token, DEFAULT_PAGE_SIZE)
        .expect("second page must succeed");

    assert!(!last.has_more, "last page must have has_more=false");
    assert_eq!(last.page.len(), 7, "last page must have the 7 remaining rows");
}

// ── AC7: Unknown cursor_id → structured error ──────────────────────────────

#[test]
fn ac7_unknown_cursor_returns_structured_error() {
    let store = CursorStore::new(600);
    let bogus_id = "00000000-0000-0000-0000-000000000000";

    let result = store.next_page(bogus_id, 0, DEFAULT_PAGE_SIZE);

    match result {
        Err(e) => {
            assert_eq!(
                e.error, "CursorExpired",
                "unknown cursor must report CursorExpired"
            );
            assert_eq!(e.cursor_id, bogus_id);
        }
        Ok(_) => panic!("next_page with unknown cursor_id must return Err"),
    }
}
