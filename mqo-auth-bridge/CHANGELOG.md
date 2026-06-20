# Changelog

## v0.5.0 — 2026-06-19

- **Execution deadline fast-fail** (PRD-mqo-execution-deadline-fast-fail):
  Every backend execution (PGWire SQL and XMLA DAX/MDX) is now wrapped in a
  configurable per-query deadline (default 60s, `--query-deadline-secs` /
  `MQO_QUERY_DEADLINE_SECS`). A breach returns a typed
  `EngineError::QueryDeadlineExceeded { elapsed_secs, deadline_secs, hint }`
  with an actionable agent hint instead of hanging until the harness wall.
  On the PGWire path `SET statement_timeout` cancels the warehouse query;
  capability fallback to client-side `tokio::time::timeout` if the backend
  rejects the GUC (FR2). The XMLA path uses `reqwest` client timeout (FR3).
  Per-request overrides clamped to `--query-deadline-max-secs` (default 120s)
  via `execute_with_deadline` + `resolve_deadline` (FR5). Zero/unparseable
  deadline falls back to 60s default with a warning (NFR2). Operator log line
  on every breach: `event=query_deadline_exceeded backend=… elapsed=…` (FR7).
  6 new AC7 unit tests; all existing tests updated for new `RowSource` signature.

## v0.4.1 — 2026-06-16

(patch release note placeholder)

## v0.4.0 — 2026-06-12

- **MDSCHEMA discovery** on `LiveExecutor` (PRD-mqo-live-catalog-ingestion):
  `discover_mdschema(request_type, catalog, cube, level)` mints a bearer token via
  `fetch_token_sync`, POSTs an XMLA `Discover` (Tabular), and returns the parsed
  `<row>` rowsets as `Vec<BTreeMap<String,String>>`. Used for `MDSCHEMA_MEASURES`
  (aggregator → semi_additive/is_calc), `MDSCHEMA_LEVELS` (dbtype + cardinality),
  and `MDSCHEMA_MEMBERS` (level domain). Dependency-free rowset parser
  (`parse_xmla_rows` + `xml_unescape`).

## v0.3.1 — 2026-06-11

Bump rust-toolchain channel from 1.85.0 to 1.88.0 to resolve MSRV conflict with transitive ICU/idna deps (icu_collections, idna_adapter v2.x require rustc ≥ 1.86).

## v0.3.0 — 2026-06-10

Route DAX to XMLA alongside MDX; only SQL stays on PGWire. Upgrade xmla_execute to a full soap:Envelope with Catalog and Cube in PropertyList — required by /v1/xmla for DAX EVALUATE statements. Engine::execute gains a model: Option<&str> parameter; LiveExecutor derives catalog+cube from the second and third dot-segments of the model path. Errors clearly when model is absent for a DAX/MDX dispatch (FR6). RowSource::xmla_query gains catalog+cube params to carry the values from the dispatch site to the wire. Docs in backend.rs and the LiveExecutor struct comment now match the actual routing.

## v0.2.0 — 2026-06-10

replace XMLA synthetic-shape fallback with real parser: Tabular rowset and MDDataSet cellset formats parsed to Vec<Value> rows; SOAP Fault → Err (no fabricated rows); numeric cells parse to JSON numbers, absent/empty to Value::Null (BLANK≠0); respects row limit.
