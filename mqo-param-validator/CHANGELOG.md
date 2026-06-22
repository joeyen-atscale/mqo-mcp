# Changelog

## v0.19.0 — 2026-06-22

RULE 13: `RedundantCoGrainSiblingLevel` guard (PRD-mqo-redundant-cograin-sibling-level-guard). Closes the `web-sales-per-customer-state` mqo-eval residual (row_recall 0.0): the agent bound two co-grain sibling levels of one hierarchy — `Customer State` (the code) and `Customer State Name` — in the `dimensions` slot. Both are valid catalog levels at the same cardinality, so RULES 8/10/11/12 stayed silent; but projecting both adds a redundant column that changes the row shape and corrupts the measure roll-up (replay: 2-level Texas 9.31B vs single-level 9.42B).

New `RejectReason::RedundantCoGrainSiblingLevel { dimension, hierarchy, level_keep, level_drop }` variant. `check_redundant_cograin_sibling()` pairs dimension refs that resolve to the SAME hierarchy and share a co-grain *core label* — equal after stripping a single trailing `{name, code, key, id, description, desc}` qualifier (`cograin_core_label`, generalizing RULE 9's name-only drop). Keeps the descriptive (name) level, recommends dropping the sibling (`cograin_choose_keep_drop`). Conservative (FR5): fires only when both labels are real levels of that catalog hierarchy; genuine drill pairs (Country + State) have different cores and never fire. Operator off-switch (NFR3): `MQO_VALIDATOR_DISABLE_RULE13=1`. 10 new unit tests in `rule13_tests` (AC1 witness, AC2 order-insensitive, AC3/AC3b genuine drill silent, AC4 single-level silent, AC5 cross-hierarchy silent, AC6 unresolved-level silent, AC7 off-switch, core-label helper).

## v0.16.0 — 2026-06-20

SQL validator iter-2: two new SQL-text rules in `sql_validator.rs`. (1) **NonSelectStatement** (`non_select_statement`): rejects DML/DDL that does not begin with SELECT or WITH — catches single-statement UPDATE/DELETE/DROP/INSERT before it reaches the warehouse. (2) **WindowFunctionInSelect** (`window_function_in_select`): rejects SQL containing `OVER (` outside string/comment context — AtScale's MQO compiler never emits window functions; their presence indicates the agent injected a synthetic rank column; message guides to ORDER BY + LIMIT. Window check is suppressed when NonSelectStatement already fired. 37 sql_validator tests pass (23 new). (PRD-mcp-server-validator-migration iter-2)

## v0.15.0 — 2026-06-20

SQL multi-statement validator: rejects multi-statement SQL before execution (R-MS rule). Server wiring in mqo-mcp-server pipeline.rs. New `sql_validator` module with `SqlRule::MultiStatement` enum, `SqlRejection` typed error, and `validate_sql()` function. `PipelineError::SqlRejected` variant added to server pipeline for clean typed rejection before warehouse execution. 14 regression tests covering multi-statement variants, semicolons-in-strings (admit case), empty SQL. No stack-trace text in rejections. (PRD-mcp-server-validator-migration iter-1)

## v0.14.0 — 2026-06-19

RULE 9: NonLocalDimensionPath guard (PRD-mqo-store-local-dimension-path-preference). Fixes `store-quantity-sold-per-brand` grouping by generic `Product Brand Name` instead of store-local `Store Item Product Brand Name`, causing ~26% per-brand inflation (row_recall 0.0, silent wrong numbers).

New `CatalogHierarchy.fact_local_facts: Vec<String>` field (empty = conformed; non-empty = fact-local to those facts, derived from FactBindings). New `RejectReason::NonLocalDimensionPath { measure, generic_level, suggested_level, fact }` variant. `check_nonlocal_dimension_path()` fires when a fact-local (single-channel) measure is grouped by a generic conformed level that has a fact-local sibling on a hierarchy bound to the measure's fact. Guard stays silent for all-channel measures (FR5) and when no fact-local sibling exists (FR4). `r9_core_label()` strips scope qualifiers (store/item/catalog/web) for robust sibling matching. 6 new unit tests in `tests/rule9_nonlocal_dim_path.rs` (AC1–AC4, AC5 suggestion one-step, AC6 no-scope silent, coexistence guardrail). All 142 tests pass; cargo clippy clean.

## v0.13.0 — 2026-06-19

PRD-mqo-unique-name-bracket-label-guard acceptance-criteria tests. The PRD's bracket-label guard (Path B of `check_non_canonical_level_label`) was already implemented in v0.9.2; this version adds 4 explicitly PRD-named tests tracing AC1 (Floor Space bracket corrected unique_name), AC5 (Number of Employees bracket corrected unique_name), AC7/FR5 (zero-match bracket defers to Unmapped, RULE 8 silent), and AC8/FR7 (level + bracket same canonical → exactly 1 rejection). All 66 unit tests pass; cargo clippy clean. Cloudbuild unavailable (MISSING); local cargo used (fallback per instructions).

## v0.12.0 — 2026-06-19

## v0.11.0 — 2026-06-18

RULE 12 role-confusion guard (PRD-mqo-grounding-enforcement-dedup): rejects MQO params where a measure name appears in the `dimensions` slot or a level name appears in the `measures` slot. Catalog-driven check; fires only when the name resolves unambiguously to exactly one kind (ambiguous names and unresolved names are deferred to the binder). New `RejectReason::RoleConfusion { entity, actual_kind, correct_slot }` variant. New `check_role_confusion()` function wired after RULE 11 in `validate()`. 5 new tests (AC1–AC5): measure-as-dim fired, level-as-measure fired, ambiguous silent, correct usage silent, unresolved silent.

## v0.10.0 — 2026-06-18

RULE 6 dimension-scoped rank grounding (PRD-mqo-rule6-dimension-scoped-rank-grounding): bracket-label level grounding is now scoped to the referenced dimension. Closes the cross-dimension grounding leak where a foreign dimension's `Rank` level caused RULE 6 to accept synthetic rank columns in unrelated queries (4 C9 rank-persist cases). New shared helper `dimension_levels_for_prefix(catalog, prefix)` → `Option<Vec<String>>` resolves a bracket prefix to the owning dimension's levels; conservative flat-union fallback when prefix is unresolvable (FR5). Measure grounding remains catalog-global (FR4). New RULE 6 tests: AC1 cross-dim leak fires, AC2 in-dim grounded silent, AC5 unresolvable-prefix conservative.

RULE 10 ambiguous-level-by-dimension resolution (PRD-mqo-validator-ambiguous-level-dimension-resolution): fires when a level label suffix-matches ≥2 catalog levels globally (RULE 8 declines) but exactly one candidate belongs to the dimension the ref names. New `RejectReason::AmbiguousLevelResolvedByDimension { supplied, canonical, dimension }` variant. New helper `suffix_candidates_with_dim(candidate, catalog)` exposes the full `(label, dim)` candidate set used by both RULE 8 (via existing `unique_suffix_match`) and RULE 10. Wired in `validate` after RULE 8. New tests: AC1 state-name customer-dim, AC2 brand-name product-dim, AC3/AC5 no-double-fire with RULE 8.

RULE 11 fuzzy near-miss level guard (PRD-mqo-validator-fuzzy-near-miss-level-guard): last-resort dimension-scoped fuzzy correction using the existing `strsim::jaro_winkler`. Fires only when RULE 8 and RULE 10 both declined (no suffix match exists) AND exactly one dimension-local level is within `NEAR_MISS_JW_THRESHOLD = 0.90`. New `RejectReason::NearMissLevelLabel { supplied, canonical, similarity }` variant. New tests: AC1 Warehouse-Square-Footage fires, AC2 exact-label silent, AC3 two-near-matches silent, AC6 unresolvable-prefix silent.

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
