# Changelog

## v0.3.1 — 2026-06-11

Bump rust-toolchain channel from 1.85.0 to 1.88.0 to resolve MSRV conflict with transitive ICU/idna deps (icu_collections, idna_adapter v2.x require rustc ≥ 1.86).

## v0.3.0 — 2026-06-10

Route DAX to XMLA alongside MDX; only SQL stays on PGWire. Upgrade xmla_execute to a full soap:Envelope with Catalog and Cube in PropertyList — required by /v1/xmla for DAX EVALUATE statements. Engine::execute gains a model: Option<&str> parameter; LiveExecutor derives catalog+cube from the second and third dot-segments of the model path. Errors clearly when model is absent for a DAX/MDX dispatch (FR6). RowSource::xmla_query gains catalog+cube params to carry the values from the dispatch site to the wire. Docs in backend.rs and the LiveExecutor struct comment now match the actual routing.

## v0.2.0 — 2026-06-10

replace XMLA synthetic-shape fallback with real parser: Tabular rowset and MDDataSet cellset formats parsed to Vec<Value> rows; SOAP Fault → Err (no fabricated rows); numeric cells parse to JSON numbers, absent/empty to Value::Null (BLANK≠0); respects row limit.
