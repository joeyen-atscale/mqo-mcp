# mqoguard-null-path-detector

Post-execution discriminator that flags all-NULL cross-fact results as `path_incompatible`.

## TL;DR

When an agent submits a measure×dimension combination that spans incompatible facts,
`mqo-mcp-server` executes and returns rows whose measure values are all NULL. In the
mcp-tuner k=4 run (2026-06-09), agents accepted this output as a valid answer on **44 of 400
rollouts** — the single largest contributor to the `path_incompatible` mode's 22.5% path-mean.

This library is a **post-execution discriminator**: given a result set plus the enriched catalog,
it decides whether an all-NULL result on a cross-fact path is a *failure to be signaled* versus a
*legitimate empty/zero result to be reported as data* — and never confuses the two.

## Verdict types

```rust
pub enum PathVerdict {
    Ok,                          // at least one non-NULL measure value
    PathIncompatible { .. },     // all-NULL + disjoint column-groups
    EmptyButValid,               // all-NULL (or zero rows) on a compatible path
}
```

## Usage

```rust
use mqoguard_null_path_detector::{classify, BoundMqo, QueryResult};
use mqoguard_column_group_enrichment::{enrich, CatalogSnapshot, FactBindings};

let bindings = FactBindings::tpcds_defaults();
let catalog = enrich(&CatalogSnapshot::default(), &bindings);
let mqo = BoundMqo { measure_names: vec!["tpcds.inv_quantity_on_hand".into()], dimension_names: vec!["tpcds.d.promotion.name".into()] };
let result = QueryResult { rows: vec![/* ... */], measure_columns: vec![] };

match classify(&result, &mqo, &catalog) {
    PathVerdict::PathIncompatible { disjoint_groups, .. } => {
        // Signal to agent: this path is impossible
    }
    PathVerdict::EmptyButValid => { /* legitimate empty result */ }
    PathVerdict::Ok => { /* real data returned */ }
}
```

## Algorithm

1. **FR3** — any non-NULL measure value → `Ok` immediately.
2. Collect column-groups for the MQO's measures and dimensions from the enriched catalog.
3. **NFR2 (conservative)** — if either group set is missing, return `Ok` (never fabricate `PathIncompatible`).
4. If measure-groups and dimension-groups are **disjoint** → `PathIncompatible`.
5. Otherwise → `EmptyButValid`.

## Guarantees

- **Zero false positives** on legitimate empty/zero results (the hard gate per the PRD).
- Pure, total, no panics, no LLM, no network.
- Zero `unsafe` code.
- Under 10 ms per result at the server row cap.

## Depends on

- [`mqoguard-column-group-enrichment`](../mqoguard-column-group-enrichment) — provides `EnrichedCatalog`.

## Status

`v0.1.0` — iter-1 scaffold. All 6 ACs and 11 tests passing.
