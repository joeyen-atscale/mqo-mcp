# mqo-session-footprint-meter

**Classify `mqo-mcp-server` session bytes by token class** — the measurement
instrument for ATSCALE-49215 (context optimization) and ATSCALE-49212
(result-set persistence).

## TL;DR

ATSCALE-49215 and ATSCALE-49212 both require a measured baseline: "baseline
context size measured for TPC-DS and SE-DEMO models" and "context-window
footprint of a 10-turn session measured before and after; target >80%
reduction."  This CLI drives a real `mqo-mcp-server` stdio session (or replays
a recorded fixture), classifies every JSON-RPC byte into five context classes,
and emits `session_footprint.json` whose per-class token counts are the
before/after ruler both spikes require.

Token cost uses `ceil(chars / chars_per_token)` — identical to
`slai-context-budget-profiler` — so the two tools agree by construction.

## Context classes

| Class | Description |
|---|---|
| `system_prompt` | MCP `initialize` / `tools/list` envelope |
| `catalog_describe_model` | `describe_model` response body (catalog content) |
| `tool_call` | Request frames + non-rows response envelopes |
| `tool_result_rows` | The `rows` array in `query_multidimensional` responses |
| `dialogue` | Any assistant-visible text not attributed elsewhere |

## Usage

```sh
# Offline fixture replay (no cluster required):
mqo-session-footprint --fixture my_session.json --format json

# With catalog section detail:
mqo-session-footprint --fixture my_session.json --with-section-detail

# Markdown output:
mqo-session-footprint --fixture my_session.json --format markdown

# Adjust token estimate:
mqo-session-footprint --fixture my_session.json --chars-per-token 3
```

Fixture file format — a JSON array of `{op, payload}` objects:
```json
[
  {"op": "system",           "payload": "{\"jsonrpc\":\"2.0\",\"id\":0,\"result\":{\"capabilities\":{}}}"},
  {"op": "describe_model",   "payload": "..."},
  {"op": "request",          "payload": "..."},
  {"op": "query_multidimensional", "payload": "..."}
]
```

## Security (AC5)

`--server` invocations containing `--pg-pass` (literal password) are **rejected
with exit 2**.  Always use `--pg-pass-env ATSCALE_PG_PASS` to pass credentials
via environment variable.

## Output shape

```json
{
  "model": "tpcds_benchmark_model",
  "turns": 10,
  "chars_per_token": 4,
  "total_tokens": 41230,
  "classes": {
    "system_prompt": 2100,
    "catalog_describe_model": 18400,
    "tool_call": 1850,
    "tool_result_rows": 17900,
    "dialogue": 980
  },
  "catalog_sections": { "measures": 6200, "dimensions": 7400 },
  "per_turn": [{ "turn": 1, "op": "describe_model", "tokens": 18400 }]
}
```

`sum(classes) == total_tokens` always holds — no double-counting.

## Acceptance criteria

| AC | Status | Description |
|---|---|---|
| AC1 | MUST | `total_tokens` = per-frame token sum; `sum(classes)` = `total_tokens` exactly |
| AC2 | MUST | Query responses split: `tool_result_rows` > `tool_call` for 5000-row result |
| AC3 | MUST | `describe_model` → `catalog_describe_model`; `catalog_sections` sum consistent |
| AC4 | MUST | Same fixture + same `cpt` = byte-identical; lower `cpt` raises all counts |
| AC5 | MUST | Rejects `--pg-pass` literal in `--server`; exit 2 + diagnostic |
| AC6 | SHOULD | Live smoke gated on `ATSCALE_PGWIRE_HOST` + `ATSCALE_PG_PASS`; skips green when absent |
| AC7 | MUST | `cargo test` offline; `cargo clippy --all-targets -- -D warnings` clean |

## Install

```sh
# From source:
cargo build --release
install -Dm755 target/release/mqo-session-footprint ~/.local/bin/mqo-session-footprint

# Verify:
mqo-session-footprint --version
```

## Dependencies

- `serde` + `serde_json` — JSON serialization
- `clap` — CLI argument parsing
- No LLM, no live network in the library — the `mqo-mcp-server` subprocess owns the cluster connection

## Related tools

- [`slai-context-budget-profiler`](https://github.com/joeyen-atscale/slai-context-budget-profiler) — sizes a single `describe_model` JSON; shares the `chars/4` token convention
- `mqo-mcp-server` — the `stdio` JSON-RPC server this meter drives
