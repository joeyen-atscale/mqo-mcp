//! Core `Engine` trait and `EngineResult` type.

use serde_json::Value;

use crate::{backend::Backend, error::EngineError};

/// Hard upper bound on rows any engine implementation will ever emit.
/// Keeps results bounded by construction — matches the fixture engine's cap.
pub const HARD_ROW_CAP: usize = 1000;

/// The bounded result of executing a compiled query.
///
/// Field-identical to `mqo-mcp-server`'s `engine::EngineResult` so the swap
/// from fixture to live is mechanical.
#[derive(Debug, Clone)]
pub struct EngineResult {
    /// Result rows as JSON objects (column name → value).
    pub rows: Vec<Value>,
    /// Set to `true` when the result was clamped to [`HARD_ROW_CAP`].
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

    /// Construct a result that **was** clamped to the row cap.
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
    /// rows (further bounded by [`HARD_ROW_CAP`]).
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
