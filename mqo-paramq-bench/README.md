# mqo-paramq-bench

Offline t-bench: free-form vs structured MQO pass@k over the mcp-tuner failure-mode corpus.

## TL;DR

ATSCALE-49213's headline AC is "a t-bench run comparing pass@k on the failure-mode corpus between
free-form `run_query` and parameterized `run_query_v2`." This CLI is that comparison rig: an
offline, replay-recorded harness that scores two arms — a free-form arm and a structured-MQO arm
(via `mqo-param-validator`) — over a failure-mode corpus, reporting first-try-valid-call rate and
pass@k per failure mode. Runs deterministically in CI with no network or LLM required.

## Acceptance Criteria

| AC | Status | Description |
|----|--------|-------------|
| AC1 | MUST | Per-failure-mode pass@1 and pass@k as fractions in [0,1] for both arms, plus overall |
| AC2 | MUST | Structured candidate with non-existent measure counted in `caught_by_validator`, not scored as path pass |
| AC3 | MUST | `lookalike_measure` task: positive `caught_by_validator` delta when structured arm rejects what free-form executes |
| AC4 | MUST | Identical path-correctness canonical-block contract across both arms |
| AC5 | MUST | first-try-valid-call rate per arm in [0,1]; no double-count of caught candidates |
| AC6 | MUST | Fully offline and deterministic — second run on same inputs is byte-identical |
| AC7 | MUST | Failure modes with zero tasks in corpus omitted from report |
| AC8 | MUST | `cargo test --release` passes; `cargo clippy --all-targets -- -D warnings` clean |

## Install

```bash
# From source
git clone https://github.com/joeyen-atscale/mqo-paramq-bench
cd mqo-paramq-bench
cargo build --release
# Binary at target/release/mqo-paramq-bench
```

## Usage

```bash
mqo-paramq-bench \
  --corpus tpcds_failure_modes_100.json \
  --freeform-candidates freeform.json \
  --structured-candidates mqo.json \
  --catalog snapshot.json \
  --k 3 \
  --format markdown
```

### Input formats

**corpus.json** — array of tasks:
```json
[
  {
    "id": "task_001",
    "failure_mode": "lookalike_measure",
    "canonical": {
      "measures": ["[Total Sales]"],
      "dimensions": ["[Date]"],
      "rejected": ["[Total Sales Lookalike]"]
    }
  }
]
```

**freeform_candidates.json** / **structured_candidates.json** — map of task_id to ordered candidate list:
```json
{
  "task_001": [
    {
      "resolved_measures": ["[Total Sales]"],
      "resolved_dimensions": ["[Date]"],
      "mqo": { "measures": [{"unique_name": "[Total Sales]"}], "dimensions": [{"unique_name": "[Date]"}], "filters": [] }
    }
  ]
}
```

For the free-form arm `mqo` is absent. For the structured arm `mqo` is passed through `mqo-param-validator::validate`; a non-empty rejection = `caught_by_validator`.

**catalog.json** — `CatalogSnapshot` as defined by `mqo-param-validator`.

## Output (markdown)

```
# MQO Param-Q Bench Report (pass@3)

## Per Failure Mode
| Mode | Tasks | FF pass@1 | FF pass@k | St pass@1 | St pass@k | Caught | FF ftv | St ftv |
...

## Verdicts
- lookalike_measure: structured caught 18/20 the free-form arm executed

## Overall
- Tasks: 100
- Free-form  pass@1: 0.450 | pass@k: 0.720 | first-try-valid: 0.850
- Structured pass@1: 0.550 | pass@k: 0.780 | first-try-valid: 0.920
- Total caught by validator: 28
```

## Dependencies

- [`mqo-param-validator`](https://github.com/joeyen-atscale/mqo-param-validator) — server-side MQO field validator
- `clap`, `serde`, `serde_json`
