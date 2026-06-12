# mqo-bench-history

Longitudinal regression tracker for `mqo-bench` runs.

**TL;DR:** `mqo-bench` proves MQO beats text-to-SQL per run, but regressions are invisible over time. This CLI ingests `mqo-bench` JSON output into an append-only JSONL history store, computes a rolling baseline, and emits a regression report when any headline metric moves unfavorably vs the trailing-5-run average.

## Usage

### Ingest a bench run

```bash
mqo-bench-history ingest bench-output.json
```

Options:
- `--history-file <path>` — where to store history (default: `~/.local/share/mqo-bench-history/runs.jsonl`)
- `--baseline-window <N>` — number of prior runs to average for baseline (default: 5)
- `--regress-threshold <pp>` — pp drop to trigger REGRESS verdict (default: 5.0)

Exit code: 0 if OK/WARN, 1 if any metric REGRESS.

### View history report

```bash
mqo-bench-history report
mqo-bench-history report --last 20
mqo-bench-history report --csv
```

Options:
- `--last <N>` — how many recent runs to show (default: 10)
- `--csv` — emit RFC-4180 CSV with header
- `--history-file <path>` — same as ingest

## Metrics tracked

| Metric | Regression trigger |
|--------|--------------------|
| `accuracy_delta_pp` | drops > threshold pp vs baseline |
| `entity_error_delta_pp` | rises > threshold pp vs baseline |
| `latency_delta_ms` | increases vs baseline |
| `token_delta` | increases vs baseline |

## Input schema

Reads JSON emitted by `mqo-bench --output json`:

```json
{
  "aggregate": {
    "accuracy_delta_pp": 82.5,
    "entity_error_delta_pp": -7.3,
    "latency_delta_ms": -450.0,
    "token_delta": -120.0
  },
  "per_question": [...]
}
```

## CI integration

```bash
mqo-bench --output json > bench-output.json
mqo-bench-history ingest bench-output.json || echo "REGRESSION DETECTED"
```
