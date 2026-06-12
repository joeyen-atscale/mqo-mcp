# mcp-spike-evidence-bundle

Maps grooming spike ACs to produced artifacts and verdicts; emits `spike_evidence.json`.

## TL;DR

The four grooming spikes (ATSCALE-49212/49213/49214/49215) each carry numbered
acceptance criteria, and the MQO MCP fleet produces an artifact for most of them —
a `session_footprint.json`, a paramq bench report, a handle-store demo, a walkthrough
transcript. But a pile of JSON files isn't a hand-back; engineering needs "here is AC1,
here is the number, here is what's still a gap."

This CLI is the capstone: it ingests the artifacts the sibling PRDs emit, maps each
ticket's ACs to `{the artifact that answers it, a verdict — produced | gap |
skip-needs-live}`, and emits one `spike_evidence.json` plus a per-ticket markdown brief.
It is the document taken back to the grooming thread: a single source of truth for
which spike ACs are answered today, which need a live DAX/MDX port or a live LLM,
and which are genuinely unbuilt.

## Depends on

These four sibling projects produce the artifact JSON this CLI consumes:

- [`mqo-session-footprint-meter`](../mqo-session-footprint-meter/) → `session_footprint.json`
- [`mqo-paramq-bench`](../mqo-paramq-bench/) → `bench_report.json`
- [`mqo-handle-walkthrough`](../mqo-handle-walkthrough/) → `walkthrough.json`
- [`mqo-duckdb-handle-store`](../mqo-duckdb-handle-store/) → `handle_demo.json`

## Install

```
cargo install --path .
```

## Usage

```
mcp-spike-evidence \
  --ticket-map fixtures/ticket_map.json \
  --footprint path/to/session_footprint.json \
  --paramq path/to/bench_report.json \
  --walkthrough path/to/walkthrough.json \
  --handle-demo path/to/handle_demo.json

# Markdown hand-back brief:
mcp-spike-evidence \
  --ticket-map fixtures/ticket_map.json \
  --footprint path/to/session_footprint.json \
  --paramq path/to/bench_report.json \
  --format markdown
```

All artifact arguments are optional. Missing artifacts degrade their ACs to
`gap` or `skip-needs-live` (as configured in the ticket-map) — never a crash.

Exit 0 on any honest gap; exit 2 on malformed input.

## Acceptance criteria

| AC | Level | Test |
|---|---|---|
| AC1 | MUST | All four inputs + ticket-map → `spike_evidence.json` with one entry per ticket, per-AC verdicts | `tests/acceptance_AC1.rs` |
| AC2 | MUST | Present value → `produced` with value_summary; absent → `gap`/`skip-needs-live` (never `produced`) | `tests/acceptance_AC2.rs` |
| AC3 | MUST | `skip-needs-live` carries non-empty `blocked_on`; run exits 0 | `tests/acceptance_AC3.rs` |
| AC4 | MUST | Per-ticket summary counts sum to AC count (no AC dropped, no double-count) | `tests/acceptance_AC4.rs` |
| AC5 | MUST | `--format markdown` → one section per ticket, `skip-needs-live` rows render `blocked_on` | `tests/acceptance_AC5.rs` |
| AC6 | MUST | Malformed JSON → exit 2 + stderr naming bad file; missing optional artifact → degrade, not crash | `tests/acceptance_AC6.rs` |
| AC7 | MUST | `cargo test` passes offline on fixtures; `cargo clippy --all-targets -- -D warnings` clean | `tests/acceptance_AC7.rs` |

## Non-goals

- Producing any measurement itself (reads sibling outputs only)
- Deciding whether a spike should ship
- Inventing AC coverage the artifacts don't support (missing = gap, never produced)
- Network access or live DAX/MDX port execution

## License

MIT OR Apache-2.0
