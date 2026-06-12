//! Cursor / pagination protocol for `query_multidimensional`.
//!
//! When a query result exceeds `PAGE_SIZE` rows the full result is persisted in
//! `CursorStore` (backed by `mqo-duckdb-handle-store::MemStore`) and a first
//! page is returned with a `cursor_id`.  Subsequent pages are fetched via the
//! `next_page` tool which resolves the `cursor_id` → `DatasetHandle` and reads
//! the requested slice from the store.
//!
//! The `cursor_id` IS the `DatasetHandle` UUID string — thin wrapper, no extra
//! mapping table needed.

use mqo_duckdb_handle_store::{
    mem_store::MemStoreConfig, ColumnSchema, DatasetHandle, MemStore, ResultStore, StoreError,
};
use serde_json::Value;
use std::sync::Mutex;

/// Token serialised in the first-page response.  Subsequent `next_page` calls
/// pass `page_token` back verbatim; the server treats it as a byte offset.
pub type PageToken = usize;

/// Default page size (rows per page).  Configurable via `--page-size`.
pub const DEFAULT_PAGE_SIZE: usize = 50;

/// Default cursor TTL in seconds.  Configurable via `--cursor-ttl-secs`.
pub const DEFAULT_CURSOR_TTL_SECS: u64 = 600;

/// A first-page response returned when rows exceed the threshold.
#[derive(Debug, serde::Serialize)]
pub struct CursorFirstPage {
    /// Opaque cursor identifier (= the underlying `DatasetHandle` UUID).
    pub cursor_id: String,
    /// Number of rows in each page (may be smaller for the last page).
    pub page_size: usize,
    /// Total rows in the full result set.
    pub total_rows: usize,
    /// The first `page_size` rows.
    pub page: Vec<Value>,
    /// Token to pass as `page_token` on the next `next_page` call.
    /// For the first page this equals `page.len()`.
    pub page_token: PageToken,
    /// `true` when there are more pages available.
    pub has_more: bool,
}

/// A subsequent-page response returned by `next_page`.
#[derive(Debug, serde::Serialize)]
pub struct CursorPage {
    pub cursor_id: String,
    /// The rows in this page.
    pub page: Vec<Value>,
    /// Token to pass on the *next* `next_page` call.
    pub page_token: PageToken,
    /// `true` when there are still more pages.
    pub has_more: bool,
}

/// Structured error returned by `next_page` when the cursor is missing/expired.
#[derive(Debug, serde::Serialize)]
pub struct CursorError {
    pub error: String,
    pub cursor_id: String,
}

/// Thread-safe cursor store wrapping a `MemStore`.
///
/// Constructed once at server startup with the configured TTL.  Shared across
/// all requests via `Arc<CursorStore>`.
pub struct CursorStore {
    inner: Mutex<MemStore>,
    ttl_secs: u64,
}

impl CursorStore {
    /// Create a new store with the given TTL and no total-row cap.
    #[must_use]
    pub fn new(ttl_secs: u64) -> Self {
        let config = MemStoreConfig {
            ttl_secs,
            total_row_cap: 0,
        };
        CursorStore {
            inner: Mutex::new(MemStore::new(config)),
            ttl_secs,
        }
    }

    /// Store `rows` and return a `CursorFirstPage` with the first `page_size` rows.
    ///
    /// The `cursor_id` in the response is the UUID string of the allocated
    /// `DatasetHandle`.  Eviction of expired entries runs before every `put`.
    ///
    /// # Errors
    ///
    /// Returns `StoreError` if the backing store write fails.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (only possible after a panic on
    /// another thread while holding the lock, which is not expected in normal use).
    pub fn put_and_first_page(
        &self,
        rows: Vec<Value>,
        page_size: usize,
    ) -> Result<CursorFirstPage, StoreError> {
        let now = unix_now();
        let total_rows = rows.len();
        let schema: Vec<ColumnSchema> = infer_schema(&rows);

        let envelope = {
            let mut guard = self.inner.lock().expect("cursor store mutex poisoned");
            guard.evict_expired(now);
            guard.put(&rows, &schema, now)?
        };

        let cursor_id = envelope.handle.0.clone();
        let page: Vec<Value> = rows.into_iter().take(page_size).collect();
        let page_len = page.len();
        let has_more = page_len < total_rows;

        Ok(CursorFirstPage {
            cursor_id,
            page_size,
            total_rows,
            page,
            page_token: page_len,
            has_more,
        })
    }

    /// Fetch the page at `offset` with at most `page_size` rows.
    ///
    /// Returns `Err(CursorError)` (as a serialisable value) when the handle
    /// is not found, has expired (also manifests as not-found after eviction),
    /// or the backing store errors.
    ///
    /// # Errors
    ///
    /// Returns `Err(CursorError)` with `error: "CursorExpired"` when the handle
    /// is not found or has been evicted by the TTL, or `error: "StoreError: …"`
    /// for backing-store failures.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned (only possible after a panic on
    /// another thread while holding the lock, which is not expected in normal use).
    pub fn next_page(
        &self,
        cursor_id: &str,
        offset: PageToken,
        page_size: usize,
    ) -> Result<CursorPage, CursorError> {
        let now = unix_now();
        let handle = DatasetHandle(cursor_id.to_string());

        // Evict before every read so TTL is enforced even on read paths.
        {
            let mut guard = self.inner.lock().expect("cursor store mutex poisoned");
            guard.evict_expired(now);
        }

        // Get total row count so we can compute has_more.
        let total_rows = {
            let guard = self.inner.lock().expect("cursor store mutex poisoned");
            match guard.metadata(&handle) {
                Ok(env) => env.row_count,
                Err(StoreError::HandleNotFound(_)) => {
                    return Err(CursorError {
                        error: "CursorExpired".to_string(),
                        cursor_id: cursor_id.to_string(),
                    });
                }
                Err(StoreError::BackendError(msg)) => {
                    return Err(CursorError {
                        error: format!("StoreError: {msg}"),
                        cursor_id: cursor_id.to_string(),
                    });
                }
            }
        };

        // Fetch the page slice.
        let page = {
            let guard = self.inner.lock().expect("cursor store mutex poisoned");
            match guard.get_rows(&handle, offset, page_size) {
                Ok(rows) => rows,
                Err(StoreError::HandleNotFound(_)) => {
                    return Err(CursorError {
                        error: "CursorExpired".to_string(),
                        cursor_id: cursor_id.to_string(),
                    });
                }
                Err(StoreError::BackendError(msg)) => {
                    return Err(CursorError {
                        error: format!("StoreError: {msg}"),
                        cursor_id: cursor_id.to_string(),
                    });
                }
            }
        };

        let next_token = offset + page.len();
        let has_more = next_token < total_rows;

        Ok(CursorPage {
            cursor_id: cursor_id.to_string(),
            page,
            page_token: next_token,
            has_more,
        })
    }

    /// TTL configured for this store (used by the server to report config).
    pub fn ttl_secs(&self) -> u64 {
        self.ttl_secs
    }
}

/// Return current Unix timestamp in seconds.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Derive a minimal column schema from the first row of `rows`.
///
/// When rows is empty or not an object, returns an empty schema.  This is
/// sufficient for `MemStore` which does not use the schema at runtime.
fn infer_schema(rows: &[Value]) -> Vec<ColumnSchema> {
    let Some(first) = rows.first() else {
        return vec![];
    };
    let Some(obj) = first.as_object() else {
        return vec![];
    };
    obj.keys()
        .map(|k| ColumnSchema {
            name: k.clone(),
            ty: "any".to_string(),
        })
        .collect()
}
