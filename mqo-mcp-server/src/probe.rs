//! Backend capability detection at startup.
//!
//! [`BackendCapabilities::probe`] fires trivial queries through the engine for
//! each backend (DAX, MDX, SQL) and classifies each port as:
//!
//! - [`PortStatus::Live`] — query executed without error.
//! - [`PortStatus::Rejected`] — the cluster returned a query error (port is
//!   reachable but rejected the statement — SSDAX/XMLA not licensed, etc.).
//! - [`PortStatus::Unreachable`] — TCP/TLS/auth failure; the port is not up.
//!
//! The probe results are used in [`BackendCapabilities::effective_backend`] to
//! downgrade the router's backend selection to SQL when the preferred backend is
//! not live, and to surface an error when all three are dead.

use mqo_auth_bridge::{Backend, Engine, EngineError};

/// Whether a particular backend port responded correctly to a probe query.
#[derive(Debug, Clone, PartialEq)]
pub enum PortStatus {
    /// The probe query executed without error.
    Live,
    /// The port is reachable but rejected the query
    /// (e.g. DAX/MDX feature not licensed).
    Rejected {
        /// Human-readable reason from the engine error.
        reason: String,
    },
    /// The port could not be reached at all (TCP, TLS, or auth failure).
    Unreachable {
        /// Human-readable reason from the engine error.
        reason: String,
    },
}

impl std::fmt::Display for PortStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Live => write!(f, "live"),
            Self::Rejected { reason } => write!(f, "rejected({reason})"),
            Self::Unreachable { reason } => write!(f, "unreachable({reason})"),
        }
    }
}

/// Capability status for all three backends, as determined by [`BackendCapabilities::probe`].
#[derive(Debug, Clone)]
pub struct BackendCapabilities {
    /// Status of the DAX backend (`PGWire` path).
    pub dax: PortStatus,
    /// Status of the MDX backend (XMLA path).
    pub mdx: PortStatus,
    /// Status of the SQL backend (`PGWire` path).
    pub sql: PortStatus,
}

impl BackendCapabilities {
    /// All three backends reported as [`PortStatus::Live`] — useful as a
    /// default when no probe is run (fixture mode).
    #[must_use]
    pub fn all_live() -> Self {
        Self {
            dax: PortStatus::Live,
            mdx: PortStatus::Live,
            sql: PortStatus::Live,
        }
    }

    /// Classify an [`EngineError`] as [`PortStatus::Rejected`] or
    /// [`PortStatus::Unreachable`].
    ///
    /// - Connection / auth / transport failures → `Unreachable`
    /// - Query errors → `Rejected` (the port is live but rejected the statement)
    fn classify_error(e: &EngineError) -> PortStatus {
        match e {
            EngineError::QueryError { reason } => PortStatus::Rejected {
                reason: reason.clone(),
            },
            other => PortStatus::Unreachable {
                reason: other.to_string(),
            },
        }
    }

    /// Run trivial probe queries through `engine` for each backend and return a
    /// [`BackendCapabilities`] struct describing what is live.
    ///
    /// Probe queries:
    /// - **DAX / MDX** — `DBSCHEMA_CATALOGS` Discover request via
    ///   [`Engine::ping_xmla`].  Model-agnostic: no catalog or cube is required.
    ///   Both backends share the same XMLA endpoint so one Discover covers both.
    /// - **SQL** — `SELECT SUM("<measure>") FROM "atscale_catalogs"."<schema>"."<model>" LIMIT 1`
    ///   using the first measure's label and the schema / model derived from
    ///   `catalog_snapshot`.  A FROM-less `SELECT 1` is **not** used because
    ///   `AtScale` `PGWire` rejects FROM-less queries.
    ///
    /// Errors are classified via [`Self::classify_error`]; a successful result
    /// (even an empty row set) marks the backend [`PortStatus::Live`].
    #[must_use]
    pub fn probe(
        engine: &dyn Engine,
        catalog_snapshot: Option<&serde_json::Value>,
        _xmla_model_coords: &std::collections::HashMap<String, (String, String)>,
    ) -> Self {
        // ── Derive probe parameters from the catalog snapshot ──────────────
        let (schema, model_name, measure_label) = extract_probe_params(catalog_snapshot);

        // ── DAX / MDX probe ────────────────────────────────────────────────
        // A single DBSCHEMA_CATALOGS Discover verifies XMLA liveness for both
        // backends — no model path is required.
        let xmla_alive = match engine.ping_xmla() {
            Ok(()) => PortStatus::Live,
            Err(e) => Self::classify_error(&e),
        };
        let dax = xmla_alive.clone();
        let mdx = xmla_alive;

        // ── SQL probe ──────────────────────────────────────────────────────
        // We must include a FROM clause — AtScale PGWire rejects FROM-less queries.
        let sql_query = format!(
            "SELECT SUM(\"{measure_label}\") FROM \"atscale_catalogs\".\"{schema}\".\"{model_name}\" LIMIT 1"
        );
        let sql = match engine.execute(&sql_query, Backend::Sql, Some(1), None) {
            Ok(_) => PortStatus::Live,
            Err(e) => Self::classify_error(&e),
        };

        Self { dax, mdx, sql }
    }

    /// Determine the effective backend for query routing.
    ///
    /// - If `requested` is [`PortStatus::Live`] → return `Some(requested)`.
    /// - If `requested` is not live but SQL is live → return `Some(Backend::Sql)`
    ///   (downgrade to SQL).
    /// - If SQL is also not live → return `None` (all useful paths are dead).
    #[must_use]
    pub fn effective_backend(&self, requested: Backend) -> Option<Backend> {
        let requested_status = match requested {
            Backend::Dax => &self.dax,
            Backend::Mdx => &self.mdx,
            Backend::Sql => &self.sql,
        };

        if *requested_status == PortStatus::Live {
            return Some(requested);
        }

        // Requested backend is not live; try SQL fallback.
        if self.sql == PortStatus::Live {
            return Some(Backend::Sql);
        }

        // All useful backends are dead.
        None
    }

    /// Return a human-readable reason string explaining why `requested` was
    /// downgraded, for use in log messages.
    #[must_use]
    pub fn downgrade_reason(&self, requested: Backend) -> String {
        let status = match requested {
            Backend::Dax => &self.dax,
            Backend::Mdx => &self.mdx,
            Backend::Sql => &self.sql,
        };
        status.to_string()
    }
}

/// Extract `(schema, model_name, measure_label)` from a catalog snapshot JSON value.
///
/// - `schema` — top-level `"schema"` field (e.g. `"tpcds_Snowflake"`).
/// - `model_name` — prefix of the first measure's `unique_name` before the
///   first dot (e.g. `"tpcds_benchmark_model"` from
///   `"tpcds_benchmark_model.total_store_sales"`).
/// - `measure_label` — `"label"` field of the first column with `kind == "measure"`.
///
/// Falls back to `("probe_schema", "probe_model", "probe_measure")` when the
/// snapshot is absent or the required fields are not present.
fn extract_probe_params(catalog: Option<&serde_json::Value>) -> (String, String, String) {
    let Some(cat) = catalog else {
        return (
            "probe_schema".to_string(),
            "probe_model".to_string(),
            "probe_measure".to_string(),
        );
    };

    let schema = cat
        .get("schema")
        .and_then(|s| s.as_str())
        .unwrap_or("probe_schema")
        .to_string();

    let measure_col = cat
        .get("columns")
        .and_then(|c| c.as_array())
        .and_then(|arr| {
            arr.iter().find(|col| {
                col.get("kind").and_then(|k| k.as_str()) == Some("measure")
            })
        });

    let measure_label = measure_col
        .and_then(|col| col.get("label").and_then(|l| l.as_str()))
        .unwrap_or("probe_measure")
        .to_string();

    // Model name from the unique_name prefix: "tpcds_benchmark_model.revenue" → "tpcds_benchmark_model"
    let model_name = measure_col
        .and_then(|col| col.get("unique_name").and_then(|u| u.as_str()))
        .and_then(|un| un.split('.').next())
        .filter(|s| !s.is_empty())
        .unwrap_or("probe_model")
        .to_string();

    (schema, model_name, measure_label)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mqo_auth_bridge::{Backend, EngineError, EngineResult};
    use serde_json::json;

    // ── Configurable fake engine for tests ──────────────────────────────────

    /// A fake engine whose XMLA-ping and SQL responses are independently configured.
    ///
    /// DAX and MDX now share a single XMLA endpoint probed via `ping_xmla`;
    /// the old per-backend `execute` path is only used for actual SQL queries.
    struct FakeEngine {
        /// Result returned by `ping_xmla` (covers DAX + MDX probe).
        xmla_ping: Result<(), EngineError>,
        /// Result returned by `execute(…, Backend::Sql, …)`.
        sql: Result<EngineResult, EngineError>,
    }

    impl FakeEngine {
        fn new(
            xmla_ping: Result<(), EngineError>,
            sql: Result<EngineResult, EngineError>,
        ) -> Self {
            Self { xmla_ping, sql }
        }

        fn live_rows() -> EngineResult {
            EngineResult::new(vec![json!({"probe": 1})])
        }

        fn rejected() -> EngineError {
            EngineError::QueryError {
                reason: "feature not licensed".to_string(),
            }
        }

        fn unreachable() -> EngineError {
            EngineError::ConnectionFailure {
                reason: "connection refused".to_string(),
            }
        }
    }

    impl Engine for FakeEngine {
        fn ping_xmla(&self) -> Result<(), EngineError> {
            match &self.xmla_ping {
                Ok(()) => Ok(()),
                Err(EngineError::QueryError { reason }) => Err(EngineError::QueryError {
                    reason: reason.clone(),
                }),
                Err(e) => Err(EngineError::ConnectionFailure {
                    reason: e.to_string(),
                }),
            }
        }

        fn execute(
            &self,
            _query: &str,
            backend: Backend,
            _limit: Option<u64>,
            _model: Option<&str>,
        ) -> Result<EngineResult, EngineError> {
            match backend {
                Backend::Sql => match &self.sql {
                    Ok(_) => Ok(Self::live_rows()),
                    Err(EngineError::QueryError { reason }) => Err(EngineError::QueryError {
                        reason: reason.clone(),
                    }),
                    Err(e) => Err(EngineError::ConnectionFailure {
                        reason: e.to_string(),
                    }),
                },
                // DAX/MDX execute path is used for real queries, not probing.
                Backend::Dax | Backend::Mdx => Ok(Self::live_rows()),
            }
        }
    }

    // ── probe tests ─────────────────────────────────────────────────────────

    /// `probe_1`: XMLA unreachable, SQL live → DAX=Unreachable, MDX=Unreachable, SQL=Live.
    ///
    /// DAX and MDX share the XMLA endpoint; a single `ping_xmla` result applies to both.
    #[test]
    fn probe_1_xmla_unreachable_sql_live() {
        let engine = FakeEngine::new(
            Err(FakeEngine::unreachable()), // XMLA probe: unreachable → DAX+MDX Unreachable
            Ok(FakeEngine::live_rows()),    // SQL: live
        );
        let caps = BackendCapabilities::probe(&engine, None, &std::collections::HashMap::new());

        assert!(
            matches!(caps.dax, PortStatus::Unreachable { .. }),
            "DAX should be Unreachable, got {:?}",
            caps.dax
        );
        assert!(
            matches!(caps.mdx, PortStatus::Unreachable { .. }),
            "MDX should be Unreachable, got {:?}",
            caps.mdx
        );
        assert_eq!(caps.sql, PortStatus::Live, "SQL should be Live");
    }

    /// `probe_2`: XMLA dead, SQL live → DAX-routed query is downgraded to SQL.
    #[test]
    fn probe_2_dax_routed_query_downgraded_to_sql() {
        let engine = FakeEngine::new(
            Err(FakeEngine::unreachable()), // XMLA dead
            Ok(FakeEngine::live_rows()),    // SQL live
        );
        let caps = BackendCapabilities::probe(&engine, None, &std::collections::HashMap::new());

        let effective = caps.effective_backend(Backend::Dax);
        assert_eq!(
            effective,
            Some(Backend::Sql),
            "DAX not live → downgrade to SQL"
        );
    }

    /// `probe_3`: When all backends are Live, no downgrade occurs.
    #[test]
    fn probe_3_all_live_no_downgrade() {
        let engine = FakeEngine::new(
            Ok(()),                      // XMLA live → DAX + MDX live
            Ok(FakeEngine::live_rows()), // SQL live
        );
        let caps = BackendCapabilities::probe(&engine, None, &std::collections::HashMap::new());

        assert_eq!(caps.effective_backend(Backend::Dax), Some(Backend::Dax));
        assert_eq!(caps.effective_backend(Backend::Mdx), Some(Backend::Mdx));
        assert_eq!(caps.effective_backend(Backend::Sql), Some(Backend::Sql));
    }

    /// `probe_4`: When XMLA is rejected and SQL is dead, `effective_backend` returns `None`.
    #[test]
    fn probe_4_all_dead_returns_none() {
        let engine = FakeEngine::new(
            Err(FakeEngine::rejected()),    // XMLA rejected → DAX+MDX Rejected
            Err(FakeEngine::unreachable()), // SQL also dead
        );
        let caps = BackendCapabilities::probe(&engine, None, &std::collections::HashMap::new());

        assert_eq!(
            caps.effective_backend(Backend::Dax),
            None,
            "no fallback when SQL is dead too"
        );
        assert_eq!(
            caps.effective_backend(Backend::Mdx),
            None,
            "no fallback when SQL is dead"
        );
    }

    /// `probe_5`: SQL=Live means SQL stays SQL (no further downgrade).
    #[test]
    fn probe_5_sql_live_not_downgraded_further() {
        let engine = FakeEngine::new(
            Err(FakeEngine::rejected()),  // XMLA dead
            Ok(FakeEngine::live_rows()), // SQL live
        );
        let caps = BackendCapabilities::probe(&engine, None, &std::collections::HashMap::new());

        let effective = caps.effective_backend(Backend::Sql);
        assert_eq!(
            effective,
            Some(Backend::Sql),
            "SQL stays SQL when SQL is live"
        );
    }

    // ── extract_probe_params tests ──────────────────────────────────────────

    #[test]
    fn probe_params_from_catalog_snapshot() {
        let catalog = json!({
            "schema": "tpcds_Snowflake",
            "columns": [
                { "unique_name": "sales.revenue", "label": "Revenue", "kind": "measure" },
                { "unique_name": "time.calendar.[Year]", "label": "Year", "kind": "level" }
            ]
        });
        let (schema, model, measure) = extract_probe_params(Some(&catalog));
        assert_eq!(schema, "tpcds_Snowflake");
        assert_eq!(model, "sales");
        assert_eq!(measure, "Revenue");
    }

    #[test]
    fn probe_params_fallback_when_no_catalog() {
        let (schema, model, measure) = extract_probe_params(None);
        assert_eq!(schema, "probe_schema");
        assert_eq!(model, "probe_model");
        assert_eq!(measure, "probe_measure");
    }

    // ── display tests ────────────────────────────────────────────────────────

    #[test]
    fn port_status_display() {
        assert_eq!(PortStatus::Live.to_string(), "live");
        assert_eq!(
            PortStatus::Rejected {
                reason: "not licensed".to_string()
            }
            .to_string(),
            "rejected(not licensed)"
        );
        assert_eq!(
            PortStatus::Unreachable {
                reason: "refused".to_string()
            }
            .to_string(),
            "unreachable(refused)"
        );
    }

    // ── all_live helper ──────────────────────────────────────────────────────

    #[test]
    fn all_live_returns_live_for_all_backends() {
        let caps = BackendCapabilities::all_live();
        assert_eq!(caps.dax, PortStatus::Live);
        assert_eq!(caps.mdx, PortStatus::Live);
        assert_eq!(caps.sql, PortStatus::Live);
    }
}
