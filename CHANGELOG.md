# Changelog

## v0.30.0

### mqo-mcp-server: large-result handle-first contract + dataset_export tool

**FR-1 — Handle-first large-result response:**
When `query_multidimensional` produces a result exceeding the page-size threshold
the response now leads with `{handle, row_count, columns, sample, notes}` instead
of presenting the result as rows+cursor-for-more. The cursor fields
(`cursor_id`, `page`, `page_token`, `has_more`) are retained for back-compat.
The `notes` field explicitly steers the LLM toward `dataset_*` ops or
`dataset_export` rather than looping `next_page`.

**FR-2 — `dataset_export` MCP tool:**
New tool exposing the `dh-export` library as the deliberate, audited
materialization boundary:
- `format="json"`: returns rows inline, bounded by `max_rows`
  (default + cap: `DEFAULT_EXPORT_MAX_ROWS = 10_000`). Above the cap returns
  typed `result_too_large {row_count, cap}` — no rows.
- `format="csv"` / `"parquet"`: writes a file to `destination` (or a temp path),
  returns `{path, row_count, bytes, sha256}` — no rows inlined.

**FR-5 — Tool descriptions updated:**
`query_multidimensional` and `next_page` descriptions explicitly state the
handle-first contract for large results and discourage `next_page` looping.

**FR-6 — Back-compat:**
Small results, `next_page`, and all `dataset_*` ops are unchanged.

<!-- Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com> -->
