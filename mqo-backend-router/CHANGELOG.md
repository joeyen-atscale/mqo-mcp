# Changelog

## v0.4.0 — 2026-06-21

Type-aware SQL member-filter quoting (PRD-mqo-sql-backend-member-filter-type-quoting). `filter_to_sql` now renders `Filter::MemberLevel` and `Filter::Member` literals by the level's catalog `value_type` instead of unconditionally single-quoting: numeric-typed levels (integer/decimal family) emit bare literals (`"Income Band" IN (9)`), while string/date/untyped levels keep the prior single-quoted `''`-escaped form. A non-numeric value on a numeric level falls back to quoting (never emits a malformed bare token). `CatalogContext` gains a `value_types` map populated from each column's `value_type` in `from_json`, mirroring the already-type-aware `Filter::Range` path. Fixes CE `db error` on numeric member filters routed through the SQL backend (e.g. `WHERE "Income Band" IN ('9')` → `IN (9)`). Four new unit tests: numeric single/multi bare, untyped quoted (unchanged), non-numeric-on-numeric quoted fallback.

## v0.3.0 — 2026-06-19

ORDER BY clause for the SQL backend: `build_sql_projection` now translates `mqo.order` into a SQL ORDER BY clause inserted between the WHERE clause (if any) and the LIMIT clause. Each sort key resolves its column name via `CatalogContext.labels` when available, falling back to `quote_last_segment`. Direction values map to `ASC` / `DESC`. When `mqo.order` is absent or empty, no ORDER BY is emitted (backwards-compatible). Three new unit tests cover: ORDER BY before LIMIT, absent order → no clause, and catalog-label resolution in sort keys.

## v0.2.0 — 2026-06-10

Limit-aware backend routing: compare `min(estimate_rows(...), mqo.limit)` against `row_threshold` so bounded queries (limit ≤ threshold) never route to SQL solely via a large cardinality cross-product. Unbounded high-cardinality extracts (no limit) continue to route to SQL unchanged. When limit caps the routing decision, `RoutingDecision.reason` states the raw estimate and limit value for operator audit. No change to the `RoutingDecision` JSON shape, the cardinality formula, or the shape-flag→MDX priority.
