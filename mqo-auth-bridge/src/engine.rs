//! Core `Engine` trait and `EngineResult` type.

use serde_json::Value;

use crate::{backend::Backend, error::EngineError};

/// Default per-handle **materialization budget**: the maximum number of rows
/// the bridge will fetch, truncate to, and persist into a handle when the
/// operator does not pass `--max-result-rows`.
///
/// This is the persisted-handle ceiling (PRD-mqo-handle-full-materialization),
/// decoupled from the inline-sample bound the server uses for the LLM context.
/// Raising it makes a handle a faithful proxy for the full result; it does NOT
/// enlarge what `query_multidimensional` inlines (that is `inline_threshold`).
pub const DEFAULT_MAX_RESULT_ROWS: usize = 50_000;

/// Upstream `PGWire` ceiling. The materialization budget must never exceed this —
/// the bridge cannot promise more rows than the engine will deliver (NFR-1).
pub const MAX_RESULT_ROWS_CEILING: usize = 200_000;

/// Deprecated alias retained for one release (migration §8). Historically a
/// hard-coded 1000-row clamp applied at execution time *before* the handle was
/// persisted, silently truncating every large result. It is now the default
/// materialization budget; direct referents keep compiling. Prefer
/// [`DEFAULT_MAX_RESULT_ROWS`] / the configured budget on `EndpointConfig`.
pub const HARD_ROW_CAP: usize = DEFAULT_MAX_RESULT_ROWS;

/// The bounded result of executing a compiled query.
///
/// Field-identical to `mqo-mcp-server`'s `engine::EngineResult` so the swap
/// from fixture to live is mechanical.
#[derive(Debug, Clone)]
pub struct EngineResult {
    /// Result rows as JSON objects (column name → value).
    pub rows: Vec<Value>,
    /// Set to `true` when the real result **exceeded the materialization budget**
    /// and was therefore truncated to it. A consumer MUST treat a tripped result
    /// as incomplete (surface a typed over-budget signal), never as the full
    /// answer (PRD-mqo-handle-full-materialization, FR-3).
    pub row_cap_tripped: bool,
}

impl EngineResult {
    /// Construct a result that has not been capped.
    #[must_use]
    pub fn new(rows: Vec<Value>) -> Self {
        Self {
            rows,
            row_cap_tripped: false,
        }
    }

    /// Construct a result that **was** clamped to the materialization budget
    /// (the real result exceeded the budget).
    #[must_use]
    pub fn capped(rows: Vec<Value>) -> Self {
        Self {
            rows,
            row_cap_tripped: true,
        }
    }
}

/// The abstraction the MQO MCP server programs against.
///
/// Both [`crate::FixtureEngine`] (deterministic, cluster-free) and
/// [`crate::LiveExecutor`] (real `PGWire` / XMLA) implement this trait.
pub trait Engine {
    /// Execute `compiled_query` against `backend`, returning at most `limit`
    /// rows (further bounded by the executor's materialization budget; see
    /// `EndpointConfig::max_result_rows`).
    ///
    /// `model` is required for XMLA dispatch (`Dax`/`Mdx`) to derive the
    /// catalog and cube from the dot-separated MQO model path.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] for auth failures, connection failures, missing
    /// env vars, or any other structured error condition.
    fn execute(
        &self,
        compiled_query: &str,
        backend: Backend,
        limit: Option<u64>,
        model: Option<&str>,
    ) -> Result<EngineResult, EngineError>;

    /// Probe the XMLA endpoint without executing a query.
    ///
    /// Sends a `DBSCHEMA_CATALOGS` Discover request to verify that the endpoint
    /// is reachable and authenticated.  Does not require a model path, catalog,
    /// or cube — the Discover request is model-agnostic.
    ///
    /// The default implementation returns `Ok(())` — fixture engines are always
    /// considered live for probing purposes.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] if the endpoint is unreachable or returns a
    /// non-200 HTTP status.
    fn ping_xmla(&self) -> Result<(), EngineError> {
        Ok(())
    }
}
