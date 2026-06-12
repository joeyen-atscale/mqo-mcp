# Changelog

## v0.13.0 — 2026-06-11

Merge the dataset-handle capability into the one canonical server
(PRD-mqo-mcp-handle-merge). `mqo-mcp-server` is now the union: live execution +
catalog + cursor + federation + charts + the full handle-op family.

- **Store/op kernel swap.** `handle_ops.rs` is re-backed by **`dh-store` +
  `dh-ops`** (typed columnar), replacing the `mqo-duckdb-handle-store` `MemStore`
  Rust-over-`serde_json::Value` op path. `dh-store`/`dh-ops`/`dh-summary`/
  `dh-export` added as path deps. The bundled DuckDB C++ build is **not** enabled
  (`libduckdb-sys` stays out of the binary).
- **Size-gated `query_multidimensional`.** Always returns
  `{summary, handle, capabilities, row_count}`; raw `rows` are inlined **only**
  when `row_count <= inline_threshold`. New `--inline-threshold` launch flag,
  default **25**. Above K: a handle + bounded summary and **no** row dump — the
  structural anti-calculator guarantee.
- **Full 10-op `dataset_*` family.** Adds `dataset_filter`, `dataset_sort`,
  `dataset_top_n`, `dataset_pivot`, `dataset_compare`, `dataset_drill`,
  `dataset_describe` alongside the existing `dataset_aggregate`,
  `dataset_slice`, `dataset_period_over_period`, `dataset_chart`. All carry
  `readOnlyHint: true`. Tool count: 23 (12 core + 11 dataset ops).
- **Compatibility.** Existing tools unchanged for ≤K results. The live
  bind→route→compile→execute path is untouched; only the result handling
  (store + size-gate) changed. `dataset_aggregate` still accepts the legacy
  `measures:[{col,agg}]` shape; `dataset_slice` remains a `[{col,op,value}]`
  filter alias.

Remaining (follow-on): cursor `next_page` still uses the separate MemStore-backed
cursor store (one-store unification deferred); the dh-ops-vs-DuckDB differential
test is stubbed (test-only DuckDB dev-dep deferred to keep the build gate fast).

## v0.11.0 — 2026-06-11

Add end-to-end functional test suites (`tests/e2e_scenarios.rs` and `tests/binary_stdio_test.rs`). Seven NLQ→BI scenario tests and four binary stdio JSON-RPC tests covering all 13 acceptance criteria from PRD-mqo-mcp-e2e-functional-tests. Fix tool count assertion from 14 to 16 (now includes `build_bi_asset`, `compose_dashboard`, and four handle-ops tools). All 43 tests pass.

## v0.6.2 — 2026-06-10

DAX-primary via `/v1/xmla` + live parity gate. `--xmla-url` now derives
`https://<endpoint-host>/v1/xmla` (the engine's HTTP XMLA path) when omitted, so
DAX/MDX reach the engine without the operator memorizing the URL; an
underivable/empty URL is no longer silently kept — DAX/MDX queries fail with a
structured error naming `/v1/xmla` instead of POSTing to an empty URL. Help/doc
guidance flips from SQL-only to DAX-primary: `--force-backend sql` is documented
as an explicit fallback, not a requirement on mcp-aws. Adds a gated live
cross-backend parity test (`Total Store Sales` over `/v1/xmla` must equal
`10,169,858,384.28` within tolerance; skip-with-log when creds/network absent)
and a secrets-discipline test asserting no raw-secret flags exist (env-var-name
flags only). Secrets read only from env (`ATSCALE_OIDC_SECRET`,
`ATSCALE_PG_PASS`); never written to disk. Fixture mode remains the cluster-free
default.

## v0.5.0 — 2026-06-10

add dataset_aggregate, dataset_slice, dataset_period_over_period, dataset_chart MCP handle-op tools with INLINE_THRESHOLD=20 head-sample cap

## v0.4.0 — 2026-06-10

Add recommend_chart and build_vega_spec MCP tools that wire mqo-result-profiler + mqo-chart-recommender + mqo-vega-emitter into the server's tools/list surface. Both tools are read-only, deterministic, and idempotent.

## v0.3.0 — 2026-06-10

add backend capability probe: BackendCapabilities::probe() classifies each backend as Live/Rejected/Unreachable via the existing Engine; routing auto-downgrades to SQL for dead backends; --no-probe escape hatch; replaces blunt --force-backend sql requirement.

## v0.2.0

Wire `mqo-auth-bridge` live executor into the server. Adds `--endpoint`/`--xmla-url`
+ OIDC flags; presence selects `LiveExecutor`, absence (default) retains `FixtureEngine`
so cluster-free CI is unchanged. Fail-fast OIDC auth check at startup. Secret passed
only via `--oidc-client-secret-env <VARNAME>` (never a flag value). MCP wire contract
(`tools/list`, `readOnlyHint`, `query_multidimensional` schema) unchanged.
