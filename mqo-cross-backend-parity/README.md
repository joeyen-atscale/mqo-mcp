# mqo-cross-backend-parity

**PRD:** PRD-mqo-cross-backend-parity  
Parity oracle: proves one MQO returns the same answer on DAX, MDX, and SQL backends.

## TL;DR

Given an MQO and a set of backends (DAX, MDX, SQL), this crate executes the MQO on each,
compares the results using DAX-semantic equality via `pbicorr-dax-result-comparator`, and
emits a structured parity report.  A mismatch exits non-zero; all skipped exits zero.

## Acceptance Criteria

1. Single backend: trivially Agree (no pairs to compare → `AllSkipped`).
2. Two backends with identical rows: `Equal` verdict, overall `Agree`.
3. Float values differing within tolerance: `WithinTolerance` verdict.
4. Two backends with row count mismatch: `Mismatch` verdict, overall `Mismatch`.
5. Skipped backend does not affect overall verdict: overall `AllSkipped` (run succeeds).
6. `ParityReport::build` computes `OverallVerdict` automatically from pairs.

All 15 tests pass; clippy clean (`-D warnings`).

## Install

Add to `Cargo.toml`:

```toml
mqo-cross-backend-parity = { git = "https://github.com/joeyen-atscale/mqo-cross-backend-parity", branch = "master" }
```

Or run the CLI:

```sh
cargo run --bin mqo-parity -- --mqo query.json --catalog catalog.json --backends dax,sql
```

## Architecture

- `src/comparator.rs` — `DaxComparator` (production, backed by `pbicorr-dax-result-comparator`)
  and `StubComparator` (unit tests).
- `src/lib.rs` — `run_parity`, `ParityReport`, `BackendStatus`, `PairVerdict`, `OverallVerdict`.
- `src/main.rs` — CLI entry point (`mqo-parity`).

The `ResultComparator` trait decouples the comparison engine from the orchestration logic;
swapping comparators is a one-line change per call site.
