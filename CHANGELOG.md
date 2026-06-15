# Changelog

## [0.41.0] - 2026-06-15

### Added
- **Attribute-aggregation guard (RULE 5)** (`mqo-param-validator` +
  `mqo-mcp-server`): `dataset_aggregate` now rejects, before execution,
  any call where the `measure` argument resolves unambiguously to a catalog
  dimension level (`kind=level`) and to no measure.  This eliminates the
  silent-wrong-number failure class (e.g. `sum_Store Number of Employees`)
  that produced plausible-looking but semantically incorrect results.
  - New `RejectReason::AttributeAggregation { column, reason }` variant with
    a corrective suggestion ("project or select the attribute; don't aggregate
    it").
  - New public `check_dataset_aggregate_attribute(col, group_by, catalog)` in
    `mqo-param-validator` — pure, deterministic, zero I/O, ≤2 ms p99.
  - Conservative fail-open predicate: unknown column, ambiguous match (also a
    measure), empty `group_by`, or absent catalog snapshot → no rejection
    (FR-2 zero false positives).
  - `handle_dataset_aggregate` gains an optional `catalog: Option<&Value>`
    parameter; production path passes `Some(&self.catalog)`; test fixtures pass
    `None` (unchanged behavior for callers without a catalog).
  - 5 new unit tests in `mqo-param-validator`: level rejected, real measure
    passes, unknown/empty-group-by/ambiguous all fail-open.
  - Target: `store-employee-counts` pass^4 → 4/4 (baseline 0/4 @ v0.37.0).

## [0.40.0] - 2026-06-15

### Fixed
- **Projection over-cap returns a handle, not a rejection** (PRD-mqo-projection-handle-over-cap):
  - **FR-1 (cap → budget alignment):** `DEFAULT_MAX_PROJECTION_CARDINALITY` now equals
    `DEFAULT_MAX_RESULT_ROWS` (50,000). In live mode, `effective_max_projection` is sourced
    from the clamped `--max-result-rows` so the projection cap and the materialization budget
    are the same single knob. Rollback: `--max-result-rows 10000`.
  - **FR-2/FR-3 (within-budget → handle, not rejection):** A projection whose estimate is
    within the materialization budget proceeds; result above the inline threshold is returned
    handle-first via the existing large-result path. `projection_too_large` only fires when
    the estimate exceeds the budget.
  - **FR-4 (cross-hierarchy product advisory):** When each individual hierarchy's per-group
    estimate is ≤ cap but their cross-hierarchy product exceeds cap, the product is capped at
    the budget (advisory) and the projection proceeds. The runtime `row_cap_tripped` is the
    hard floor. Hard rejection still fires when a single hierarchy's own cardinality exceeds
    the cap. Fixes `customers-ese`: (First Name=5126) × (Gender=2) = 10,252 with a 50k budget
    — previously rejected at 10k, now proceeds as a handle.
  - **FR-5 (cross-hierarchy filter selectivity):** A `Range` filter on a non-projected
    hierarchy applies a conservative 1/10 selectivity before the per-group cap check.
    A `Member`/`MemberLevel` IN-list filter caps the estimate at the member count.
    Fixes `products-price-above-70`: `Item Product Name` (domain 206,021) with
    `Product Current Price > 70` now estimates ~20,602 (< 50k budget) instead of 206,021
    — re-enables the previously disabled corpus case.

## [0.39.0] - 2026-06-15

### Added
- **Semijoin-projection grounding** (PRD-mqo-semijoin-projection-grounding): ground the
  agent to use measureless projections (`projection:true`, `measures:[]`) with
  cross-dimension / fact-resident filters instead of fabricating an anchor measure.
  - `query_multidimensional` tool description: documents that `projection:true` with empty
    `measures` returns distinct members of the projected levels; that `filters` may include
    levels not in `dimensions` (including fact-resident levels); that the engine resolves
    such filters via SUMMARIZECOLUMNS auto-exist (semijoin); includes a worked example
    (customers-ese shape) and projection-vs-measure decision guidance.
  - `describe_model` `hierarchy_levels` entries: each level now carries
    `filterable_cross_dimension: true` so the model can discover cross-dimension
    filterability from metadata in one `describe_model` call.
  - `describe_model` response: new top-level `projection_note` field summarising the
    semijoin-projection capability once (avoids per-level repetition, NFR-2 compliant).
  - Content regression test: verifies description contains "filter", "projection", "fact"
    and every `hierarchy_levels` entry carries `filterable_cross_dimension: true`.

## [0.38.0] - 2026-06-14

- Handle full materialization (PR #15)
