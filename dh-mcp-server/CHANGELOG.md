# Changelog

## v0.2.0 — Handle-aware BI/chart tools

Extends `dh-mcp-server` with three new read-only tools that let an LLM agent
build Vega-Lite v5 charts and BI asset bundles directly from a stored
`DatasetHandle` — without the rows ever entering the context window.

**New tools:**
- `dataset_chart(handle, chart_type, x_col, y_cols, title?)` — emits a
  Vega-Lite v5 JSON spec from a stored handle using explicit column bindings.
  `readOnlyHint: true`.  No new handle created.
- `build_bi_asset(handle)` — runs the full profiler → recommender → emitter →
  bi-asset-bundle chain on the stored dataset and returns
  `{title, description, caveats, vega_spec, profile_summary}`.
  `readOnlyHint: true`.
- `compose_dashboard(handles[], title, layout?, columns?)` — builds a BI asset
  for each handle and composes them into a multi-panel Vega-Lite v5 concat spec.
  `readOnlyHint: true`.

**Capability enum additions (dh-spec):**
- `Capability::Chart` — advertised by `query_multidimensional` when the result
  has ≥1 row and ≥1 measure column.
- `Capability::BiAsset` — advertised under the same conditions.

**Implementation notes:**
- Chart crates (`mqo-result-profiler`, `mqo-chart-recommender`,
  `mqo-vega-emitter`, `mqo-bi-asset-bundle`, `mqo-dashboard-composer`) are
  consumed as workspace path dependencies — no new chart logic.
- Rows are capped at 500 before being passed to the chart crates; the spec
  (not the rows) is what reaches the LLM.
- 21 tests total: 12 unit (8 new chart-tools AC tests) + 9 acceptance.
  Tool count updated from 11 → 14 in acceptance test `ac1`.

## v0.1.0 — Initial release

`query_multidimensional` + 10 `dataset_*` handle ops.
