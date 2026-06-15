# Changelog

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
