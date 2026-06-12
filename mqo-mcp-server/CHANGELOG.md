# Changelog

## v0.16.0 — 2026-06-12

- **describe_model grounding fixes (k=1 residual gaps).** Two focused fixes to
  the disambiguation pack, closing the residual failure causes from the k=1
  grounding eval (after `near_twins` lifted wrong_hierarchy_level 35%→60% and
  lookalike 85%→90%):
  - **FIX 1 — packaged calcs surfaced on measures.** Each `describe_model`
    measure column now carries `is_calc: bool` and `triggers: [String]`,
    reusing `mqo-param-validator`'s `is_packaged_calc()` / `calc_triggers()`.
    Packaged calc measures (e.g. `Web and Catalog Sales Price Growth`,
    `Store Sales Increase`) are flagged `is_calc:true` with their NL trigger
    phrases (`growth`, `year over year`, `yoy`, `vs prior period`, …) so the
    model picks the calc instead of a plain base measure. Non-calc measures get
    `is_calc:false` + `triggers:[]`. (Failing tasks fm2-001, fm2-015.)
  - **FIX 2 — `canonical_for` prefers the human-readable `*Name*` attribute.**
    The near-twin core-label key now drops a trailing `name` token, so a
    code-like attribute (`Customer State`) and its display sibling
    (`Customer State Name`) land in one group; within a group the member whose
    label ends in `Name` wins `canonical_for`, with the prior
    shortest-hierarchy primacy as the tiebreak. The "Customer State" group now
    resolves to `Customer State Name`, while the Brand Name group still resolves
    to `product_dimension.[Product Brand Name]`. The measure-twin pass gains the
    same conservative `*Name*` preference (inert on the TPC-DS fixture).
    (Failing tasks fm2-008, fm2-010, fm2-020.)

## v0.15.0 — 2026-06-11

- **wire grounding: describe_model surfaces level+measure near-twins,
  param-validator in query path, enriched-catalog date-role binding.** Three
  already-built grounding capabilities now fire at runtime:
  - **describe_model near_twins now populates for real.** A model-scoped
    `describe_model` call previously dropped every dimension level (levels live
    under dimension-prefixed unique_names, not the fact-cube prefix), so
    `build_near_twins` saw nothing and `near_twins` was always empty. The
    level-twin pass now reads levels from the full catalog regardless of the
    `model` filter, and a new measure-twin pass groups lookalike measures by
    their concept tail across fact-group prefixes (Catalog/Store/Web/Total).
    Each group is tagged `twin_kind: "level" | "measure"`. Over the TPC-DS
    fixture this yields 60 groups (44 level-twin + 16 measure-twin) — e.g. Brand
    Name across 3 product hierarchies and "sales price" across the sales-channel
    measure groups.
  - **param-validator wired into the query path.** After `mqo_spec::validate`
    and before binding, `query_multidimensional` runs `mqo-param-validator`
    against the catalog. Grounded-but-wrong references (WrongHierarchyLevel,
    ManualCalcRederivation) are rejected pre-execution with a structured
    `param_rejected` error carrying nearest-match / `suggested_calc` hints; no
    execution happens on rejection. Unmapped references are left to the binder's
    richer `not_found` report (no behavior change for the not-ground path).
  - **enriched-catalog date-role binding active.** The server's auto-derived
    enriched catalog tags measures and date-levels with their fact
    `column_groups` and is passed to `mqo-bind` v0.3.0 via `--enriched-catalog`,
    so `bind_with_date_roles` resolves per-measure `date_role_hierarchy`
    (verified end-to-end: a store-sales measure over a sold-date axis now binds
    its date role).

## v0.14.0 — 2026-06-11

- **feat: describe_model disambiguation pack — kill near-twin entity picks.**
  `describe_model` now disambiguates the same-worded look-alike attributes that
  drove the `wrong_hierarchy_level` failure mode (65% pass@4 in mcp-tuner k4_v2 —
  the model grabbed `Store Item Product Brand Name` instead of the canonical
  `Product Brand Name`). Additive, deterministic, no extra round-trip:
  - **near_twins block (FR-2/FR-3):** dimension levels whose core label (trailing
    concept words, e.g. "brand name", "state name") collide across ≥2 hierarchies
    are grouped under a top-level `near_twins` list. Within each group the
    attribute on the shortest hierarchy name is tagged `canonical_for: "generic"`
    (hierarchy-primacy heuristic — `product_dimension` over
    `store_item_product_dimension`). TPC-DS surfaces the known conflicts: Brand
    Name (3 hierarchies), State Name (6), Day Name (4), Manager ID (3).
  - **hierarchy + level tags (FR-1):** every dimension level carries `hierarchy`
    and `level`, parsed from `hier.[Level]` when the snapshot omits them.
  - **date_roles on measures (FR-4):** each measure carries `date_roles` — the
    unique_names of temporally-typed date hierarchies (empty array when none,
    never absent). Consumed by the crossfact date-role PRD.
  - **footprint guard (NFR-2):** if the near_twins block would exceed +15% of the
    response footprint, it is trimmed to the most-actionable groups (≥3 twins).
  - 7 unit tests + 4 fixture/integration tests (AC-1..AC-5).

## v0.13.2 — 2026-06-11

- **fix: friendly-label role matching for XMLA-mangled live column keys.** For
  LIVE results the XMLA row keys are SSAS name-mangled (e.g.
  `atscale_catalogs_x005b_Sold_x0020_Calendar_x0020_Year_x005d_`) and do NOT
  equal the bound's `unique_name`
  (`sold_date_dimensions.[Sold Calendar Year]`), so the prior exact-`unique_name`
  matching found nothing and a numeric year fell through to the dtype heuristic →
  wrongly `Measure`. `bound_role_map` now classifies by **friendly label**
  (mirroring the demo bridge's `_normalize_response`): decode `_xHHHH_`, prefer
  the last `[...]` segment for both row keys and bound `unique_name`s, keep only
  bound labels present among the columns, and assign any unmatched column by
  dtype (numeric → Measure, else Dimension). The raw `unique_name` match is still
  tried first so fixture-mode rows (keys == `unique_name`) keep working. Columns
  are not renamed — only `ColumnRole` is fixed. New test
  `live_xmla_mangled_keys_role_from_bound` asserts year → Dimension, sales →
  Measure on the real mangled shapes.

## v0.13.1 — 2026-06-11

- **fix: bound-authoritative column roles — numeric dimensions no longer
  mislabeled as measures.** Query-result datasets stored in the typed handle
  store now derive each column's `ColumnRole` from the MQO `bound`
  (`measures[] → Measure`, `dimensions[] → Dimension`, keyed on `unique_name`)
  instead of the value-dtype heuristic ("numeric → Measure"). A numeric
  dimension such as "Sold Calendar Year" (returned as `Float`) is now correctly
  labelled `Dimension`, fixing `dataset_chart` and other dim-vs-measure ops on
  numeric-dimension results. Columns absent from the bound (e.g. op-derived
  columns) still fall back to the dtype heuristic. Added
  `json_rows_to_dataset_with_bound` / `HandleStore::put_rows_with_bound`; wired
  into the query-result store-put site and `attach_handle_summary`.

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
