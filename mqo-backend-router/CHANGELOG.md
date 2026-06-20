# Changelog

## v0.3.0 — 2026-06-19

ORDER BY clause for the SQL backend: `build_sql_projection` now translates `mqo.order` into a SQL ORDER BY clause inserted between the WHERE clause (if any) and the LIMIT clause. Each sort key resolves its column name via `CatalogContext.labels` when available, falling back to `quote_last_segment`. Direction values map to `ASC` / `DESC`. When `mqo.order` is absent or empty, no ORDER BY is emitted (backwards-compatible). Three new unit tests cover: ORDER BY before LIMIT, absent order → no clause, and catalog-label resolution in sort keys.

## v0.2.0 — 2026-06-10

Limit-aware backend routing: compare `min(estimate_rows(...), mqo.limit)` against `row_threshold` so bounded queries (limit ≤ threshold) never route to SQL solely via a large cardinality cross-product. Unbounded high-cardinality extracts (no limit) continue to route to SQL unchanged. When limit caps the routing decision, `RoutingDecision.reason` states the raw estimate and limit value for operator audit. No change to the `RoutingDecision` JSON shape, the cardinality formula, or the shape-flag→MDX priority.
