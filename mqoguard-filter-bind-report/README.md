# mqoguard-filter-bind-report

Reports which `Member` filters in an MQO bound successfully and which were silently dropped during compilation.

## TL;DR

When an agent puts a `Member` filter on an MQO — e.g. `{"hierarchy": "sold_date_dimensions", "members": ["2001"]}` — and the server can't bind it, the filter is **silently dropped**: the compiled SQL has no WHERE clause and the query returns all years, presented as if it were the 2001 answer. This library makes filter binding explicit: every `query_multidimensional` response reports `filters_applied[]` and `filters_dropped[]`, each dropped filter with a typed reason.

## Problem

Dropped filters are indistinguishable from applied ones in today's MCP response. In the 2026-06-09 mcp-tuner run (k=4), year filters were silently discarded across many tasks, confounding `wrong_date_role` (50.0% path-mean) and `wrong_hierarchy_level` (60.0%) failure modes. The scorer couldn't separate "agent chose the wrong path" from "agent chose the right path but the filter was silently discarded."

## API

```rust
use mqoguard_filter_bind_report::{FilterBindReport, report_filters};

let report: FilterBindReport = report_filters(&mqo_json, &compiled_sql_json);
// report.filters_applied: Vec<AppliedFilter>
// report.filters_dropped: Vec<DroppedFilter>  — each has reason + optional suggestion
```

## Acceptance Criteria

| AC | Description |
|----|-------------|
| AC1 | Applied + dropped lists together contain every input filter, no duplicates |
| AC2 | `|applied| + |dropped| == |input_filters|` (partition invariant) |
| AC3 | Canonical dropped filter (year `"2001"` with no SQL binding) classified correctly |
| AC4 | Dropped filter with a catalog-derivable suggestion carries that suggestion |
| AC5 | Empty bound_ids → all filters dropped; binding record present → exact classification |
| AC6 | No panics on empty/unusual inputs |

## Tests

23 tests pass (3 unit + 7 AC integration + 6 AC6 robustness + 7 proptest).

```
cargo test --release
```

## Performance

Deterministic, no-LLM, no-network. Target: under 5 ms per query.

## Part of

MQO Compatibility Guardrails vision — see `mqoguard-column-group-enrichment`, `mqoguard-compatibility-matrix`, `mqoguard-regression-harness`.
