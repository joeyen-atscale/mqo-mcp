# Changelog

## v0.9.0 — 2026-06-17

Add RULE7: channel-scope mismatch guard (PRD-mqo-channel-scope-measure-grounding). Fires when the bound measure is an all-channel total and a channel-scoped sibling with the same base concept exists. Guard stays silent when no sibling exists (FR4). `ChannelScopeMismatch { measure, named_channel, suggested_measure }` RejectReason variant added. `channel_scope: Option<Vec<String>>` field added to `CatalogMeasure` (from FactBindings descriptor). `channel_family_stem` helper strips channel/qualifier tokens to detect siblings. 4 new unit tests (AC3/AC4/AC5 + absent-scope).

Add RULE6: synthetic rank/row-number guard. Rejects ungrounded rank/ordinal columns (Rank, Ranking, Row Number, RowNum, Ordinal, Position, etc.) injected by the agent into top-N queries. Grounded catalog objects named "Rank" or "Net Profit Tier" are not rejected (FR4). Fixes three eval corpus cases (store-employee-counts, store-returns-per-product, web-sales-per-customer-state) where spurious Rank column tanked column_jaccard to 0.67. Wired into validate() alongside RULE1-4,7. Add SyntheticRankColumn { column } RejectReason variant with actionable message referencing ORDER BY + LIMIT.

## v0.7.0 — Binding near-twin dimension rejection (PRD-mqo-validator-near-twin-rejection)
check_near_twin_dimension fires pre-execution and returns NonCanonicalNearTwin with the canonical suggestion. Already wired in mqo-mcp-server pipeline. Tests green.

## v0.6.0

Filter-level guard (RULE 4) — member-domain check for level-less `Member`
filters (PRD-mqo-catalog-level-domain-metadata). `mqo_spec::Filter::Member`
carries a hierarchy + member keys but no level, so the existing value-fit path
(which needs a level) never saw them. New `check_member_domain` compares each
member against the hierarchy's enumerated level domains and rejects an
out-of-domain member — but ONLY when safe: there is ≥1 enumerated same-type
domain, the member is in none, AND no same-type level lacks an enumerated domain
(a high-cardinality level the member could legitimately be a key of). This
catches a wrong code/value on a fully-enumerated dimension with zero false
positives on high-card member filters (store names, surrogate keys). The broad
"member silently bound to the wrong level" case (e.g. `Store State="CA"`) is the
binder's responsibility (no silent grounding) — tracked as a follow-on.

Honest scope: through the current MQO grammar the rule's reach is limited —
`Range` bounds are numeric (`f64`) so the type-mismatch arm is rarely reachable,
and `Member` filters never name a level. The value here is the safe member-domain
guard plus the level-domain metadata foundation (see mqo-mcp-server v0.20.0).

## v0.5.0

Semi-additive guard (RULE 2) activated and made false-positive-safe
(PRD-mqo-catalog-semi-additive-metadata). The rule was complete since v0.3.0 but
dormant because the served catalog never carried the `semi_additive` flag; the
server now plumbs it through (see mqo-mcp-server v0.19.0). Critically, the rule
now fires **only on an EXPLICIT additive override** (`sum`/`count`/`total`) — a
`None`/default aggregation on a semi-additive measure resolves to the model's
semi-additive function (last-non-empty) at the engine and is correct, so flagging
it would false-positive every legitimate "balance by period" query (e.g.
inventory-on-hand by month). New `agg_is_explicit_additive` helper; RULE 2 keys
off it instead of `agg_is_additive`. Tests updated: default-agg over time is NOT
rejected; explicit `sum` over time IS.

## v0.4.0

Path-incompatible decline guard for the `NonCanonicalNearTwin` near-twin rule
(PRD-mqo-path-incompatible-decline-guard). The near-twin canonical reroute
previously chose a canonical sibling purely by label/hierarchy structure, with no
regard for fact-compatibility with the MQO's measures — so when the requested
twin was path-incompatible with the measures but the canonical happened to be
compatible, it rerouted and the model fabricated rows on a query that should
decline (fm3-010: `Ship Customer State` → `Customer State Name`).

Before emitting a `NonCanonicalNearTwin` suggestion, the rule now checks
fact-compatibility of BOTH the picked twin and the proposed canonical against the
MQO's measures, reusing the same subject-area conformance signal already present
in the `CatalogSnapshot` (a measure's `subject_area` vs the twin hierarchy's
owning-dimension `subject_areas` — the same signal `check_cross_fact_paths`
uses). Rule:

- picked twin INCOMPATIBLE and canonical COMPATIBLE → WITHHOLD the reroute (no
  suggestion); the query proceeds to the binder which surfaces the genuine
  cross-fact incompatibility (the correct decline);
- both compatible → suggest the canonical as before (Brand Name unchanged);
- both incompatible / undeterminable → no behavior change.

CONSERVATIVE: when compatibility cannot be determined (no subject-area signal,
conformed dimension, or missing catalog entry) the rule falls back to the current
behavior, so the working Brand Name reroute is unaffected. Deterministic,
pre-execution, catalog-only — no new dependency (the compatibility signal was
already reachable from the validator's `CatalogSnapshot`).

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
