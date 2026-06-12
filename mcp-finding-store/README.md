# mcp-finding-store

Gen-10 investigation memory: JSONL finding store with supersede-on-recur.

## What it does

`mcp-finding-store` is the durable artifact store for the Gen-10 autonomous investigation loop. It keeps an append-only `findings.jsonl` of resolved investigation results, with **supersede-on-recurrence**: a fresh finding for a query whose prior is still active bumps a recurrence count instead of creating a duplicate record.

## Core types

```rust
pub enum FindingStatus { Open, Confirmed, Refuted, Escalated, Suppressed }

pub struct Finding {
    pub finding_id: String,          // "<query_id>-<uuid>"
    pub query_id: String,
    pub watch_event: serde_json::Value,         // triggering WatchEvent
    pub resolved_hypotheses: serde_json::Value, // ResolvedHypothesisSet
    pub status: FindingStatus,
    pub recurrence_count: u64,       // 0 on first sight
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
}
```

## Usage

```rust
use mcp_finding_store::{FindingStatus, FindingStore};

let store = FindingStore::open(Path::new("/var/mcp/findings"))?;

// Record a resolved investigation
let fid = store.record(
    "query-cpu-high",
    &watch_event_json,
    &resolved_json,
    FindingStatus::Open,
    now_ms,
)?;

// Supersede on recurrence — same query_id while Open bumps recurrence_count
store.record("query-cpu-high", &new_watch, &new_resolved, FindingStatus::Open, later_ms)?;

// Transition status
store.set_status(&fid, FindingStatus::Confirmed, now_ms)?;

// Query
let active = store.open_for_query("query-cpu-high")?;
let all    = store.all()?;
let by_q   = store.by_query("query-cpu-high")?;
```

## JSONL format

Each line is a JSON object with a `record_type` field:
- `"new"` — full Finding create record
- `"recur"` — supersede patch: `{ finding_id, recurrence_count, last_seen_ms, watch_event, resolved_hypotheses }`
- `"update"` — status patch: `{ finding_id, status, last_seen_ms }`

Fold-on-load applies records in order to produce latest-state per `finding_id`.  
A corrupt line causes `all()` / `get()` / etc. to return `Err` naming the line number.

## Design decisions

- **No system clock** — all time is caller-supplied `now_ms`; identical inputs produce identical state.
- **Append-only** — lines are never rewritten; supersede and status changes always add lines.
- **Active = Open | Confirmed | Refuted** — Escalated and Suppressed are terminal; a new record for the same query_id after suppression starts a fresh finding.
- **No async, no network** — pure bookkeeping, single-dir, v1.

## Acceptance criteria (7 tests)

| Test | AC |
|------|----|
| `ac1_first_sight` | First record has `recurrence_count=0`, `first_seen=last_seen=now_ms` |
| `ac2_supersede` | Second record for same open query bumps `recurrence_count` to 1 |
| `ac3_status_closes_open` | After Suppressed, new record starts fresh (recurrence_count=0) |
| `ac4_append_only` | Every operation appends exactly one line; all lines are valid JSON |
| `ac5_fold` | N appends over M findings → `all()` returns M with latest state |
| `ac6_corrupt` | Corrupt line → `Err` naming line number, no panic or silent skip |
| `ac7_deterministic` | Caller-supplied `now_ms` means identical inputs → identical state |

## Dependencies

- `serde` / `serde_json` — JSONL serialization
- `uuid` — finding_id generation
- `tempfile` (dev) — test isolation
