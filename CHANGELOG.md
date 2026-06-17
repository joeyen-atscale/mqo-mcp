# Changelog

## [0.46.0] - 2026-06-17

### Added
- **Channel scope grounding** (`mqo-mcp-server` v0.46.0, `mqo-param-validator` v0.9.0):
  surfaces channel scope metadata in `describe_model` so the agent can pick the
  channel-scoped measure (`Store Quantity Sold`) rather than the all-channel total
  (`Total Quantity Sold`) when the request names a single channel.
  - **FR1/FR2**: `describe_model` now annotates each measure with
    `channel_scope: {channel_groups: [...], channel_scope_label: "..."}` derived
    from `FactBindings::tpcds_defaults()` — the existing source of truth, no new
    hand-authored mapping.
  - **FR3/FR4/FR5** (`mqo-param-validator` RULE 7 — `ChannelScopeMismatch`): the
    validator flags an all-channel measure bound when a single-channel sibling
    exists with the same base concept. Guard stays silent when no sibling exists
    (FR4 — nothing better to suggest). Rejection names the channel-scoped sibling
    so the agent can rebind directly.
  - **pipeline**: `param_validate` now receives the channel scope map from
    `ServerEnrichedData` so RULE 7 fires at the pre-execution grounding stage.
  - **Target**: `store-quantity-sold-per-brand` binds `Store Quantity Sold`
    instead of `Total Quantity Sold`; per-brand values go from ~25% inflated
    to exact (`row_recall` baseline 0.0 → 1.0).

## [0.43.0] - 2026-06-15

### Fixed
- **Cross-hierarchy member-filter co-resolution** (`mqo-catalog-binder`):
  `MemberLevel` filters pinned to a near-twin hierarchy (e.g.
  `promotion_product_item_product_dimension.[…Product Brand Name]`) while
  projecting from a different near-twin hierarchy (e.g. `product_dimension`)
  now produce a typed `MemberUnboundCrossHierarchy` decline (exit 4,
  `member_unbound_cross_hierarchy` JSON) instead of binding silently and
  returning 0 rows — eliminating the `corpcorp-brand-products` max_steps
  thrash. The decline names the co-resolving candidate hierarchies so the
  LLM agent can retry with a consistent hierarchy in one call.
  - **FR-1**: At bind time, detects that the filter and projection hierarchies
    share ≥1 canonical attribute family (near-twin) but are different → decline.
  - **FR-2**: Preferred co-resolving hierarchy is the projection's own hierarchy
    when it also carries the filter attribute's canonical family.
  - **FR-3**: Typed `CrossHierarchyFilterError` with `candidate_hierarchies` —
    never a silent 0-row result, never a max_steps timeout.
  - **OQ-1 resolved**: `corpcorp #1` has no domain in any captured brand level
    (all `None`); correct outcome is an honest decline, not rows.
  - **Guardrail**: single-hierarchy queries (filter and projection on the same
    hierarchy) bind identically to before.

## [0.42.0] - 2026-06-15

### Added
- **Project-not-count grounding** (`mqo-mcp-server` v0.42.0, `mqo-param-validator` v0.8.0):
  targets the count-evasion failure in `store-employee-counts` where the model
  responded to RULE 5's sum-block by switching to a `count` measure instead of
  projecting the numeric attribute.
  - **FR-1 (`describe_model` flag):** hierarchy_levels entries with `kind=level`
    and a numeric `value_type` (integer/decimal/float/number) now carry
    `projectable_per_member_quantity: true` — signals to the LLM that this is a
    stored per-entity attribute that should be projected, not aggregated.
  - **FR-2 (tool description):** `query_multidimensional` description now includes
    a "Per-entity numeric attributes (projectable quantities)" section explaining
    the project-not-count pattern with a worked example (`Store Number of
    Employees`) and an explicit contrast against genuine member-count measures
    (`total_product_count`).
  - **FR-3 (validator nudge):** `check_dataset_aggregate_attribute` doc and
    rejection message updated to explicitly cover `count`/`count_distinct` in
    addition to sum/avg; rejection now includes the correct projection shape.
    Two new tests: `count_on_numeric_level_rejected` (FR-3 fire) and
    `count_measure_query_not_rejected` (FR-4 guardrail — genuine count measure
    passes through).
  - **FR-1 test:** `numeric_level_carries_projectable_per_member_quantity` —
    integer level carries the flag; string level does not.
  - **FR-2 test:** `query_multidimensional_describes_per_entity_numeric_attribute_projection`
    — tool description must mention `projectable_per_member_quantity`,
    "count rows", and "count measure".
  - Target: `store-employee-counts` projects `[Store Name, Store Number of
    Employees]` instead of `[Store Name, count_count]` on ≥3/4 reps.

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
