# mcp-probe-executor

Gen-10 probe executor: runs hypothesis probes under budget, confirm/refute by directional delta.

## TL;DR

`mcp-hypothesis-engine` proposes a ranked `HypothesisSet` — each hypothesis has a `probe_mqo` that would confirm or refute it. This binary is the wiring: it reads a `HypothesisSet`, calls `mcp-query-budget-governor::check()` before each probe, executes the probe (test mode: reads cached JSON summaries), computes a directional delta against a baseline, and emits a `ResolvedHypothesisSet`.

This is the first consumer of the Gen-9 budget governor (`mcp-query-budget-governor`). A `Halt` verdict stops the loop and records what was learned so far. Output carries mandatory `evidence_type: "structural"` and a verbatim `analysis_note` — direction confirmed, not causation.

## Usage

```sh
mcp-probe-executor \
  --hypotheses  hypset.json          \
  --budget      budget.json          \
  [--summaries  <dir>]               \
  [--baseline   <dir>]               \
  [--now-ms     <u64>]               \
  [--confirm-min-fraction 0.02]      \
  [--format     json|human]
```

### Test mode (no live server)

```sh
mcp-probe-executor \
  --hypotheses tests/fixtures/hypset_two.json \
  --budget     tests/fixtures/budget_unlimited.json \
  --summaries  tests/fixtures/summaries \
  --baseline   tests/fixtures/summaries/baseline \
  --now-ms     1000000
```

## Input shapes

**HypothesisSet** (from `mcp-hypothesis-engine`):
```json
{
  "target": "Total Store Sales",
  "hypotheses": [
    {
      "rank": 1,
      "explanation": "...",
      "probe_mqo": { "measures": [...], "dimensions": [], "filters": [] },
      "predicted_direction": "down",
      "component_delta_fraction": -0.075,
      "probe_key": "store-sales-amount"
    }
  ]
}
```

**BudgetLimits** (for `mcp-query-budget-governor`):
```json
{ "max_queries": 10, "checkin_fraction": 0.7, "max_tokens": 10000, "window_ms": 3600000 }
```

**DatasetSummary** (per probe_key in `--summaries` dir):
```json
{ "mean": 92.5, "row_count": 100 }
```

## Output

```json
{
  "target": "Total Store Sales",
  "evidence_type": "structural",
  "analysis_note": "Probes executed under budget; confirm/refute is a directional data check, not statistical causation.",
  "confirmed_count": 1,
  "refuted_count": 1,
  "checkin_pending": false,
  "halted": false,
  "resolved": [
    {
      "rank": 1,
      "explanation": "...",
      "probe_mqo": {...},
      "predicted_direction": "down",
      "observed_delta_fraction": -0.075,
      "verdict": "confirmed"
    }
  ],
  "budget": { "queries_run": 2, "verdict_at_stop": "Proceed" }
}
```

Verdict values: `confirmed`, `refuted`, `inconclusive`, `skipped_budget`.

## Budget behavior

- `Proceed` — continue
- `CheckIn` — sets `checkin_pending: true`; loop continues executing
- `Halt` — marks current and all remaining hypotheses `skipped_budget`; sets `halted: true`

## Dependencies

- `serde`, `serde_json`, `clap`
- `mcp-query-budget-governor` (path dep: `/Users/jsy/Documents/projects/mcp-query-budget-governor`)

## Tests

9 integration tests across 7 acceptance criteria (AC1–AC7):

```sh
cargo test
```
