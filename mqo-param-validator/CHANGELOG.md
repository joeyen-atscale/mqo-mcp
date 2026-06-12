## v0.3.0

Validator semantic enforcement — four new conservative pre-execution rules
(PRD-mqo-validator-semantic-enforcement vision). All ride the existing
`mqo-mcp-server` query-path wiring and surface as `param_rejected`. Every rule
is catalog-only, deterministic, and tuned for zero false rejections (pass^k
guardrail).

- RULE 1 — `RejectReason::NonCanonicalNearTwin{picked, suggested_canonical,
  group_core_label}` (PRD near-twin-rejection). Builds near-twin *dimension*
  groups from the catalog (core-label collision across ≥2 hierarchies,
  Name-preferring canonical) replicating `describe_model`'s `build_near_twins`
  heuristic, and rejects a non-canonical member, suggesting the canonical.
  Dimensions only (never measures); only groups with a clear canonical; INTENT
  GUARD: no rejection when the MQO has a filter/dimension on the picked
  member's own hierarchy.
- RULE 2 — `RejectReason::SemiAdditiveSum{measure, time_dimension,
  suggested_agg}` (PRD semi-additive-guard). Rejects an additive aggregation
  (sum/default) of a `semi_additive == true` measure over a time-typed
  dimension; suggests the catalog's declared agg (last/first/avg) or
  "average over period". DORMANT on the live fixture (the recorded snapshot
  nulls `semi_additive`); fires only on the enriched catalog. `CatalogMeasure`
  gains optional `semi_additive` / `semi_additive_agg`.
- RULE 3 — `RejectReason::CalcMisaggregation{measure, aggregation, reason}`
  (PRD calc-aggregation-guard). Rejects sum/avg of an `is_calc` measure
  classified ratio/percentage/average (new optional `calc_kind` catalog flag,
  else name signal `%`/`pct`/`rate`/`average`/`avg`). Additive calcs
  (`* increase`/`* growth`/`* delta`) and non-calc measures are never rejected.
  Reuses the crate's `is_packaged_calc` calc-detection.
- RULE 4 — `RejectReason::FilterLevelMismatch{filter, target_level, reason,
  suggested}` (PRD filter-level-check). For each filter, resolves the target
  level and rejects when the value type/domain can't match (range/member bound
  type ≠ level type; member not in an explicit level domain) or the named level
  doesn't exist (no silent grounding). An in-domain value with no live rows is
  never rejected (catalog-only; emptiness ≠ filter error). `CatalogHierarchy`
  gains optional `level_meta` (`LevelDomainMeta` with `value_type`/`domain`/
  `expected_key_shape`); DORMANT without enrichment.
- `MqoMeasureRef` gains optional `aggregation`; `MqoFilterRef` gains optional
  `members`/`range_lo`/`range_hi`. All new fields default-on-deserialize, so
  existing callers are unaffected.

## v0.2.0

Packaged-calc preference grounding (PRD-mqo-calc-preference-grounding).

- FR-1: catalog calc surfacing. `CatalogMeasure` gains optional `label` and
  `is_calc` fields. New `inspect_calcs` returns a `CalcSurfacing` per measure
  flagging packaged calcs (`is_calc: true`) with a derived `triggers` phrase
  list. Calc detection honors an explicit `is_calc: true` or falls back to a
  name heuristic (`* Increase`, `* Growth`, `* Change`, `* YoY`, `* vs Prior`,
  `* Prior`, `* Price Growth`). Helpers `is_packaged_calc` and `calc_triggers`
  exposed.
- FR-2/FR-3: new `RejectReason::ManualCalcRederivation` validator rule. Detects
  an MQO that hand-derives a packaged period-over-period calc (base series
  measure re-derived over a date axis) and rejects pre-execution, naming the
  canonical calc via the new `ParamRejection.suggested_calc` field. Fires for
  the fm5-002 (Store Sales Increase) and fm5-003 (Web Sales Increase) shapes.
- FR-5/NFR-2: conservative detection. Requires a positive re-derivation signal
  (duplicate base measure, or a lag/offset marker such as "Prior"/"lagged"/
  "ParallelPeriod") plus a date axis plus a matching packaged calc not already
  in use. Plain "base measure by date" queries and the other failure modes'
  canonical MQOs are never rejected (zero false positives).
- `ParamRejection` carries an optional `suggested_calc` (serialized only when
  present); all prior rejection paths are unchanged.
