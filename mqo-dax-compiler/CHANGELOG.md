# Changelog

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
