# Changelog

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
