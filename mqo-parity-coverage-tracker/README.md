# mqo-parity-coverage-tracker

Records parity verdicts (**verified | mismatch | never-tested**) per
(measure, backend-pair) against server builds in an append-only JSONL history
store, and computes build-over-build coverage and regression diffs.

## Why

`mqo-cross-backend-parity` emits a point-in-time `ParityReport`; nothing
stamps that result with a build id, accumulates it across runs, or computes
a denominator-based coverage number.  This tool is that missing piece: a
longitudinal parity-coverage axis that turns "1 measure verified" into a
build-over-build series and a same-build tripwire on parity regressions.

## Crate layout

```
src/
  lib.rs          — public API re-exports
  history.rs      — JSONL store schema (HistoryEntry, PairRecord, …)
  corpus.rs       — parity-corpus.v1 denominator types (Corpus, CorpusCase)
  store.rs        — record_run / load_history (append + tolerant read)
  summary.rs      — coverage_summary / delta_summary (deterministic, offline)
  main.rs         — parity-tracker CLI (record / coverage / delta / regressions)
```

## CLI

```
parity-tracker record   --input <path|-> --build-id <id> [--history <path>]
parity-tracker coverage --build-id <id> --corpus <path>  [--history <path>] [--format json|human]
parity-tracker delta    --build <N> --against <N-1> --corpus <path> [--history <path>] [--format json|human] [--no-fail]
parity-tracker regressions [--history <path>] [--format json|human]
```

Default history store: `~/.local/share/mqo-parity/history.jsonl`

### record

Ingests a `ParityReport` from `mqo-cross-backend-parity` (or per-case results
from `mqo-backend-live-harness` via `--live-harness`) and appends one entry to
the JSONL store.  The `--build-id` is **mandatory and never inferred** from the
capture, consistent with tiger-benchmarking's convention.

```bash
parity-tracker record \
  --input results/parity-run.json \
  --build-id 2026.06.10-rc1 \
  --history ./history.jsonl
```

### coverage

Computes per-(measure, backend-pair) status — `verified`, `mismatch`, or
`never-tested` — and an overall coverage % against a `parity-corpus.v1`
denominator.

```bash
parity-tracker coverage \
  --build-id 2026.06.10-rc1 \
  --corpus corpus.json \
  --history ./history.jsonl \
  --format json
```

JSON output schema (stable contract for `mqo-parity-brief` and CI):

```json
{
  "build_id": "2026.06.10-rc1",
  "corpus_version": "parity-corpus.v1",
  "verified_count": 1,
  "mismatch_count": 0,
  "never_tested_count": 2,
  "coverage_pct": 33.3,
  "denominator": 3,
  "backend_a": "dax",
  "backend_b": "sql",
  "cells": [
    { "measure": "Total Store Sales", "backend_a": "dax", "backend_b": "sql",
      "status": "verified", "ungrounded": false }
  ],
  "ungrounded_cells": []
}
```

### delta

Diffs two recorded builds and reports newly-verified, **newly-broken**, and
newly-tested measures.  Exits 2 (by default) when `newly_broken` is non-empty
so it can be used as a CI gate directly.

```bash
parity-tracker delta \
  --build 2026.06.11-rc1 \
  --against 2026.06.10-rc1 \
  --corpus corpus.json \
  --history ./history.jsonl
# exits 2 if any measure regressed (verified → mismatch)
# --no-fail to override the non-zero exit
```

## Library API

```rust
use mqo_parity_coverage_tracker::{
    record_run, load_history, coverage_summary, delta_summary,
    store::InputReport,
};

// Append a result to the store
record_run(&input_report, "2026.06.10-rc1", "my-run.json", &store_path)?;

// Load all valid entries (corrupt trailing lines are skipped with a warning)
let history = load_history(&store_path)?;

// Compute coverage
let summary = coverage_summary(&history, &corpus, "2026.06.10-rc1", None);
println!("{:.1}% parity-verified", summary.coverage_pct);

// Build-over-build diff
let delta = delta_summary(&history, &corpus, "build-N", "build-N-1", None);
if !delta.newly_broken.is_empty() { /* block ship */ }
```

## Key design decisions

- **Append-only JSONL.** No storage dependency; each line is a
  `HistoryEntry` with `schema_version` for additive evolution.
- **Backend pairs are data, not hardcoded.** `backend_a`/`backend_b` are
  strings from the verdict, so MDX pairs join later without a schema break.
- **Fully offline and deterministic.** Identical `(history, corpus, build-id)`
  inputs produce byte-identical `--format json` output (NFR1).
- **Tolerant reader.** Corrupt/partial trailing JSONL lines are skipped with a
  stderr warning; valid history is always reported (NFR2, AC-7).
- **Idempotent-safe digest.** Each entry carries a SHA-256 of
  `(build_id, sorted normalised pairs)` for duplicate detection on retry (FR5).
- **AllSkipped contributes no coverage evidence.** A result where every backend
  was down is recorded but changes no verified/mismatch counts (FR6, AC-5).

## Non-goals

- No comparison logic — consumes `PairVerdict` produced by
  `mqo-cross-backend-parity`, never re-derives it.
- No live query execution — no network, no credentials, no ports.
- No corpus generation — consumes `parity-corpus.v1` from
  `mqo-parity-corpus-gen`.
- No markdown hand-back — the Luis-facing parity brief is `mqo-parity-brief`.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | I/O or parse error |
| 2 | `delta` newly-broken list non-empty (override with `--no-fail`) |
