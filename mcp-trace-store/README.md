# mcp-trace-store

Durable, append-only interaction trace store for MQO queries.

Every MQO interaction — the question, the bind result, the grounding score, the execute
result — is stored as a JSONL record. The corpus powers the gap miner, quality scorer,
and fine-tuning exporter.

## Overview

- **Format:** JSONL, one record per line, append-only
- **Rotation:** Active file at `<path>`; rotated to `<path>.1`, `<path>.2`, etc. when
  `rotate_at_bytes` is exceeded
- **Atomic writes:** Uses `O_APPEND` for single-syscall atomicity per POSIX
- **Corrupt-line tolerance:** Corrupt or truncated lines are skipped with a warning to
  stderr; prior records are never affected
- **No external deps:** No database, no network, no async

## Usage

```rust
use mcp_trace_store::{
    BindOutcome, ExecuteOutcome, QualitySignals,
    TraceFilter, TraceRecord, TraceStore, TraceStoreConfig,
};

let cfg = TraceStoreConfig::new("~/.local/share/mcp-traces/trace.jsonl");
let store = TraceStore::new(cfg)?;

let record = TraceRecord::new(
    "session-abc",
    serde_json::json!({"entity": "Revenue"}),
    BindOutcome::Success,
    ExecuteOutcome::Success { row_count: 10, result_empty: false },
    QualitySignals {
        first_attempt_bind: true,
        bind_attempt_count: 1,
        total_latency_ms: 42,
        tokens_used: None,
    },
);
store.append(record)?;

// Query with filter
let filter = TraceFilter {
    first_attempt_only: true,
    ..TraceFilter::default()
};
let results = store.scan(&filter)?;
```

## Core types

| Type | Purpose |
|---|---|
| `TraceRecord` | Single persisted interaction |
| `BindOutcome` | `Success / Ambiguous / NotFound / Error(String)` |
| `GroundingBand` | `Grounded / Partial / Ungroundable` |
| `ExecuteOutcome` | `Success { row_count, result_empty } / Error(String) / Skipped` |
| `QualitySignals` | `first_attempt_bind`, `bind_attempt_count`, `total_latency_ms`, `tokens_used` |
| `TraceFilter` | Time range, grounding band, first-attempt, cluster, session, limit |
| `TraceStore` | `append`, `scan`, `count`, `rotate_if_needed` |

## File layout

```
<path>          ← active file (newest writes)
<path>.1        ← previous rotation (older)
<path>.2        ← two rotations ago (oldest)
```

`scan` reads fragments oldest-first (`<path>.N` descending, then `<path>`).

## Acceptance criteria

All 7 ACs pass under `cargo test` (AC7 timing SLO enforced in `--release` only):

| AC | Description |
|---|---|
| AC1 | `append` creates file and writes valid JSON lines |
| AC2 | `scan` filters by `grounding_band` correctly |
| AC3 | `scan` with `first_attempt_only` filters correctly |
| AC4 | Corrupt/partial line in JSONL does not break scan |
| AC5 | Rotation works; `scan` reads both fragments in order |
| AC6 | Missing optional fields → `None`; unknown fields ignored |
| AC7 | 10k append+scan completes in < 2s (release build) |

## License

MIT OR Apache-2.0
