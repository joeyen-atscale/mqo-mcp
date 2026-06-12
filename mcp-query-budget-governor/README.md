# mcp-query-budget-governor

Budget governor for autonomous MCP queries — Proceed/CheckIn/Halt verdict (Gen-9).

## Overview

When the semantic layer initiates its own queries autonomously (watch-daemon ticking, hypothesis-engine probing), it can spend without bound. This library is the leash: a `BudgetLedger` tracks queries run, estimated tokens, total backend latency, and wall-clock against configured limits, and `check()` returns a `Verdict` of `Proceed`, `CheckIn` (soft limit — pause and ask a human), or `Halt` (hard limit — stop now).

It reads the Gen-8 audit chain's per-query `latency_ms` to account real spend, not estimates.

## Quick start

```rust
use mcp_query_budget_governor::{BudgetLedger, BudgetLimits, Verdict};

let limits = BudgetLimits {
    max_queries: Some(100),
    max_est_tokens: None,
    max_latency_ms: Some(30_000),  // 30 seconds of backend latency
    max_wall_ms: Some(300_000),    // 5 minutes wall clock
    checkin_fraction: 0.8,         // CheckIn at 80% of any limit
};

let mut ledger = BudgetLedger::new(limits, now_ms());

// Before each unit of work:
match ledger.check(now_ms()) {
    Verdict::Proceed => { /* do work */ }
    Verdict::CheckIn { reason, fraction_used } => {
        // Pause and surface to human for approval
        eprintln!("CheckIn: {} ({:.0}% used)", reason, fraction_used * 100.0);
        return;
    }
    Verdict::Halt { reason, limit } => {
        // Stop immediately
        eprintln!("Halt: {} (limit: {})", reason, limit);
        return;
    }
}

// After work completes:
ledger.record_query(est_tokens, actual_latency_ms);
```

## Key types

### `BudgetLimits`

| Field | Type | Description |
|-------|------|-------------|
| `max_queries` | `Option<u64>` | Max queries allowed; `None` = unconstrained |
| `max_est_tokens` | `Option<u64>` | Max estimated tokens; `None` = unconstrained |
| `max_latency_ms` | `Option<u64>` | Max summed backend latency in ms; `None` = unconstrained |
| `max_wall_ms` | `Option<u64>` | Max wall-clock time since start in ms; `None` = unconstrained |
| `checkin_fraction` | `f64` | Fraction (0.0–1.0) at which CheckIn fires (e.g. `0.8` = 80%) |

### `BudgetLedger`

```rust
impl BudgetLedger {
    pub fn new(limits: BudgetLimits, started_ms: u64) -> Self;
    pub fn record_query(&mut self, est_tokens: u64, latency_ms: u64);
    pub fn ingest_audit_log(&mut self, path: &Path) -> io::Result<u64>;
    pub fn check(&self, now_ms: u64) -> Verdict;       // pure
    pub fn fraction_used(&self, now_ms: u64) -> f64;  // max across all limits
}
```

### `Verdict` precedence

`Halt` strictly dominates `CheckIn` dominates `Proceed`. When multiple limits are exceeded, the most-exceeded one names the reason.

### `agentns` module

```rust
use mcp_query_budget_governor::agentns::{read_self, CountersOutcome};

match read_self() {
    CountersOutcome::Counters(c) => { /* use kernel counters */ }
    CountersOutcome::Unsupported => { /* fall back to userspace ledger */ }
}
```

Returns `Unsupported` on macOS and any host without `/proc/self/agent_counters`. Never panics.

## Audit log ingestion

`ingest_audit_log` reads a JSONL file (one JSON object per line), sums the `latency_ms` field, and increments `queries_run`. Corrupt or non-JSON lines emit a stderr warning and are skipped — the read never aborts.

```jsonl
{"query_id":"abc","latency_ms":142,"tokens":512}
{"query_id":"def","latency_ms":98,"tokens":310}
```

## Design notes

- `check()` is pure: caller supplies `now_ms` so behavior is deterministic and testable.
- `None` limits are never constraints — a limit axis is simply ignored if unconfigured.
- `ingest_audit_log` is the only I/O operation; `check` has no side effects.
- The `agentns` module compiles everywhere; the capability check is a runtime file-exists test.
- Single-ledger, single-loop (v1). Cross-process shared budgets are out of scope.

## Dependencies

- `serde` + `serde_json` — JSONL ingestion only
- No network, no async, no `unsafe`

## Acceptance criteria

| AC | Description | Status |
|----|-------------|--------|
| AC1 | Query bands: Proceed/CheckIn/Halt at 7/8/10 of 10 | PASS |
| AC2 | Halt dominates: exceeded latency overrides 50% query usage | PASS |
| AC3 | None limits never constrain | PASS |
| AC4 | `ingest_audit_log` sums spend; corrupt lines skipped | PASS |
| AC5 | `max_wall_ms` triggers on idle loop with zero queries | PASS |
| AC6 | `check` is pure: same ledger + same `now_ms` = same Verdict | PASS |
| AC7 | `agentns::read_self()` returns `Unsupported` on macOS | PASS |
