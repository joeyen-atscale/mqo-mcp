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
