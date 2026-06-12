# Changelog

## v0.2.0 — 2026-06-10

Limit-aware backend routing: compare `min(estimate_rows(...), mqo.limit)` against `row_threshold` so bounded queries (limit ≤ threshold) never route to SQL solely via a large cardinality cross-product. Unbounded high-cardinality extracts (no limit) continue to route to SQL unchanged. When limit caps the routing decision, `RoutingDecision.reason` states the raw estimate and limit value for operator audit. No change to the `RoutingDecision` JSON shape, the cardinality formula, or the shape-flag→MDX priority.
