# mqo-handle-walkthrough

4-turn POC walkthrough asserting zero re-queries via mqo-duckdb-handle-store (Vega-Lite chart, offline fixture mode).

## TL;DR

Implements ATSCALE-49214 AC3: a runnable walkthrough of a representative 4-turn POC session (top 10 states by web sales → YoY change → drill into California → chart monthly trend) that executes entirely via handles, with exactly one `query_multidimensional` to AtScale. The re-query counter is asserted to be exactly 1 at the end — zero re-queries is a checked fact, not a claim.

## The 4-Turn Script

| Turn | Op | Description |
|------|----|-------------|
| 1 | `query` | Load seed rows (live or fixture) → `handle_A` |
| 2 | `period_over_period` | Add YoY-change column over `handle_A` → `handle_B` |
| 3 | `slice` | Filter `handle_B` to California → `handle_C` |
| 4 | `chart` | Vega-Lite line spec from `handle_C` → final artifact |

## Acceptance Criteria

- **AC1**: Default script with `--seed-result` completes all 4 turns; `requery_count == 1` in `walkthrough.json`.
- **AC2**: Turns 2–4 never increment the re-query counter; passing `requery_count=2` fails loudly.
- **AC3**: `slice` returns only California rows; input handle is unchanged (immutable-derive).
- **AC4**: `period_over_period` adds `yoy_change` column with correct delta arithmetic.
- **AC5**: `chart` emits valid Vega-Lite JSON with `$schema`/`mark`/`encoding` for a line chart.
- **AC6**: `--store mem` runs in default CI; `--store duckdb` parity under `#[cfg(feature = "duckdb")]`.
- **AC7**: Live run against mcp-aws gated behind `ATSCALE_PGWIRE_HOST` + `ATSCALE_PG_PASS`; skips green when absent.
- **AC8**: `cargo test` offline passes; `cargo clippy -- -D warnings` clean; credentials never written to any artifact.

## Install

```
git clone https://github.com/joeyen-atscale/mqo-handle-walkthrough
cd mqo-handle-walkthrough
cargo build --release
```

## Usage

```bash
# Offline (fixture):
./target/release/mqo-handle-walkthrough --seed-result fixtures/seed_result.json --out ./output

# Built-in seed (no flags required):
./target/release/mqo-handle-walkthrough --out ./output

# DuckDB backend (requires duckdb feature):
cargo run --features duckdb -- --store duckdb --out ./output

# Live arm (requires env vars):
ATSCALE_PGWIRE_HOST=<host> ATSCALE_PG_PASS=<pass> \
  ./target/release/mqo-handle-walkthrough --server mqo-mcp-server --out ./output
```

Outputs:
- `<out>/walkthrough.json` — per-turn transcript + header with `requery_count`
- `<out>/chart.vg.json` — Vega-Lite line spec for the California monthly trend

## Deps

- [`mqo-duckdb-handle-store`](https://github.com/joeyen-atscale/mqo-duckdb-handle-store) — handle store (MemStore default, DuckStore opt-in)
- `serde`, `serde_json`, `clap`
