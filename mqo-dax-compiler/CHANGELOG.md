# Changelog

## v0.18.0 — 2026-06-20

domain completeness flag + partial-domain filter diagnostic (PRD-mqo-member-filter-recall-incomplete-domain)
- DaxCatalogContext.level_domain_complete: per-level complete/incomplete flag (FR1/AC3)
- Completeness inferred from cardinality vs domain.len(); explicit flag wins; absent → false (FR2/AC3)
- is_domain_complete() accessor; partial_domain_diagnostic() emits structured operator warning (FR5/AC6)
- filter_expr_ctx: appends /* partial_domain_filter */ comment for incomplete-domain Member filters (FR5/AC6)
- 11 new unit tests covering AC3/AC4/AC6/AC7; zero regression on 122 prior tests

## v0.15.0 — 2026-06-17

String member filter: normalize whitespace/case/punctuation in resolve_member_level and check_member_filters for robust catalog lookup (fixes able-manufacturer-brands 188/246 row undercount)

## v0.14.0 — 2026-06-17

Fix projection ORDER BY keys emitting as measure refs (XMLA 300s timeout)

Previously, compile_grounded called measure_dax_ref_ctx() on dimension order
keys for measureless projection queries, producing invalid DAX that XMLA could
not resolve. The three Cycle-2 timeout cases (customer-vehicle-count-income-
band-9, customers-ese-store-2001, customer-details-new-jersey) each burned the
full 300s budget.

Changes:
- projection_topn_sort_args(): resolves all declared order keys as grounded
  dim-level column refs (level_col_ref_grounded), building a multi-key TOPN
  sort-arg list honoring each key's direction.
- append_order_by(): skips trailing ORDER BY for projection+limit (TOPN already
  sorts); uses dim refs for unbounded projection ORDER BY.
- sort_dir_str(): extracted SortDirection match.
- 6 new regression tests in projection_orderby_tests module.
- Pre-existing clippy::pedantic violations fixed in catalog_context.rs and
  codegen.rs test modules.

## v0.13.0 — 2026-06-14

Engine-validation gate (PRD-mqo-dax-engine-validation-gate): compile-time validation rejects DAX containing an `/* ungrounded */` marker or an unquoted space-bearing table identifier, naming the offending token, so a malformed-DAX regression fails the build instead of the customer's query. A CI corpus fixture (`tests/projection_gate.rs`) pins the pre-fix malformed DAX as a rejected regression case and confirms 0 false rejections on the measure-query suite. Opt-in `MQO_DAX_ENGINE_CHECK` engine-parse stub logs `engine-check-skipped` when unset.

## v0.12.0 — 2026-06-14

Projection DAX grounding fix (PRD-mqo-projection-dax-grounding): projection dimension levels and `member_level` filter levels now ground to the correct per-hierarchy physical table (`'ship_mode'[Carrier]`) instead of the catalog/database name (`'atscale_catalogs'[Carrier]`); the filter level lookup accepts both the `unique_name` and the bare label, and identifiers with spaces are single-quoted. A genuinely ungroundable level returns a typed `UngroundableLevel` decline rather than emitting `/* ungrounded */` to the engine. Live-verified: the EXPRESS-carriers projection now executes against XMLA and returns the 4 carriers matching the PGWire gold (previously engine 500 "not valid DAX").

## v0.11.0 — 2026-06-14

Lifted the EmptyMeasures guard for projection MQOs (mqo.is_projection()). The SUMMARIZECOLUMNS path at codegen.rs now emits dimension columns + filters with no measure argument for projections. SQL router emits SELECT DISTINCT. (PRD-mqo-attribute-projection)

## v0.10.0 — Real OR semantics for Filter::Group

PRD-mqo-filter-predicate-grammar: Group now compiles to ONE combined predicate
(`||` for OR-of-AND-groups, `&&` for AND-of-OR-groups) over all referenced
columns in a single FILTER(ALL(...)), replacing the v0.9.0 stub that emitted
AND semantics. Extracted filter_predicate() shared by leaf arms + groups. Two-level
nesting bound. 61 tests (1 new). Verified: marital S OR W -> single FILTER with ||.

## v0.8.0 — Member grounding declines, never first-level-fallbacks

PRD-mqo-member-grounding-decline-not-fallback. When a level-less Member filter
finds no domain match, the compiler now returns a typed UngroundedMemberFilter
decline instead of silently grounding to the hierarchy's first level (the source
of the qwf20 silent 0-row misgrounds). Safety valve: first-level fallback is kept
ONLY when the hierarchy carries no captured domains at all (hierarchy_has_any_domain),
so un-ingested deployments don't regress. 58 tests (4 new).


## v0.7.0 — 2026-06-12

- **Domain-aware Member-filter grounding** (PRD-mqo-member-filter-domain-grounding).
  Fixes the silent bug where a level-less `Member` filter bound to the hierarchy's
  FIRST level (`resolve_hierarchy_first_level`) regardless of the value — e.g.
  `customer_demographics="M"` compiled to `FILTER(ALL([Credit Rating]))` → 0 rows,
  no error. New `DaxCatalogContext.level_domains` (parsed from each level's `domain`)
  + `resolve_member_level(hierarchy, members, dim_levels)`: binds a member to the
  level whose enumerated domain contains it. Ambiguity (a value in >1 domain, e.g.
  "M" ∈ Gender ∧ Marital Status) is resolved by preferring the level the query
  groups by (dimension-preference); a sole match is used directly; otherwise it
  declines and the caller falls back to first-level (no regression). Levels without
  captured domains are unaffected.
  Verified live: `Marital Status="M"`→binds Marital Status; `Product Category=
  "Electronics"`→binds Product Category; plain queries unchanged.

## v0.5.0 — 2026-06-11

Range filter bare-label grounding: resolve bare level labels via DaxCatalogContext
reverse lookup; fail loud with UngroundedRangeFilter when unresolvable. Mirrors the
v0.3.0 member-filter fix. Also updated UngroundedMemberFilter error message to remove
the now-incorrect suggestion to use a Range filter.

## v0.4.0 — 2026-06-11

add time-intel grounding + UnsupportedTimeIntelligence guard: DaxCatalogContext gains
has_date_table + date_level_unique_name fields; compile_grounded() now grounds the
DateTable[Date] placeholder to the model's real date dim/level when a context is
present; pre-dispatch capability guard emits UnsupportedTimeIntelligence error for
ops that require Mark-as-Date-Table (YoY, PriorPeriod, ToDate, RunningTotal) when
has_date_table=false (AtScale XMLA default); no-context path is byte-identical to
pre-change behavior.

## v0.3.0 — 2026-06-11

add member-filter grounding: hierarchy_levels reverse index in DaxCatalogContext;
Filter::Member resolves hierarchy to catalog level column; EmptyMemberFilter +
UngroundedMemberFilter error variants; compile_grounded() uses grounded column refs
for member filters.

## v0.2.0 — 2026-06-10

add --catalog engine-grounding: DaxCatalogContext + compile_grounded() emit Table[Display Label] / [Measure] refs from CatalogSnapshot; compile() unchanged (backward-compat, byte-identical); unknown unique_names fall back with annotation.
