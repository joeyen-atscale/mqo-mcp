# mqo-parity-brief

Offline Rust CLI that reads the `mqo-parity-coverage-tracker` JSONL history store and
emits a Markdown parity brief for Joe's weekly update to Luis Maldonado (CPO) and
pre-grooming evidence artifacts.

**Reference cluster:** `mcp-aws.atscaleinternal.com`
**Catalog / cube:** `tpcds_Snowflake` / `tpcds_benchmark_model`

---

## Quick start

```sh
# Build (via cloudbuild wrapper):
bash ~/.claude/skills/cloudbuild/cloudbuild.sh build mqo-parity-brief --

# Run against a history file, defaulting to the most-recent build:
mqo-parity-brief --history ~/.local/share/mqo-parity/history.jsonl

# Report a specific build:
mqo-parity-brief --history history.jsonl --build-id b-2026-06-10.1

# Write to a file instead of stdout:
mqo-parity-brief --history history.jsonl --out brief.md

# Also emit a Tiger-compatible build-stamped record (FR7):
mqo-parity-brief --history history.jsonl --emit-tiger-record tiger.json

# Scope the trend to builds since a given build id:
mqo-parity-brief --history history.jsonl --since-build b-2026-06-01.1
```

---

## Inputs

| Flag | Default | Description |
|---|---|---|
| `--history <path>` | `~/.local/share/mqo-parity/history.jsonl` | Tracker JSONL history store |
| `--build-id <id>` | most-recent record in history | Build to report |
| `--since-build <id>` | (none — all builds) | Restrict trend window to builds from this id onwards |
| `--out <path>` | stdout | Write brief to file |
| `--emit-tiger-record <path>` | (none) | Also write a Tiger-compatible build-stamped record |
| `--format markdown` | `markdown` | Output format (only `markdown` in V1) |

---

## Output format

The brief is structured (oldest-first history is stable across runs — NFR1):

```
# DAX Parity Status — build `<id>` (`<version>`) on `<cluster>`

**Parity coverage: 72%** — build `b-2026-06-10.1` (`v0.3.0`) on `mcp-aws.atscaleinternal.com`

## Measures that newly disagree this build
...

## Newly verified (recovered) this build   [optional — only when non-empty]
...

## Coverage by backend pair
...

## Coverage gaps (never-tested measures)
...

## Parity coverage trend
...
```

The first content line after the title is always the headline coverage % (FR1 — inverted
pyramid). The regression section appears above per-pair breakdown and coverage gaps (FR4).

---

## JSONL history record schema

Each line in the history file is a JSON object:

```json
{
  "build_id": "b-2026-06-10.1",
  "version": "v0.3.0",
  "cluster": "mcp-aws.atscaleinternal.com",
  "recorded_at": "2026-06-10T10:00:00Z",
  "overall_verdict": "Agree",
  "measures": [
    { "measure": "Order Quantity", "backend_pair": "DAX↔SQL", "status": "verified" },
    { "measure": "Total Returns",  "backend_pair": "DAX↔SQL", "status": "mismatch" },
    { "measure": "Return Percent", "backend_pair": "DAX↔SQL", "status": "never-tested" }
  ],
  "deltas": {
    "newly_broken":   [{ "measure": "Total Returns", "backend_pair": "DAX↔SQL" }],
    "newly_verified": []
  }
}
```

Status vocabulary (NFR3): `verified` | `mismatch` | `never-tested`
Delta vocabulary (NFR3): `newly_broken` | `newly_verified`
`overall_verdict` values: `Agree` | `WithinTolerance` | `Mismatch` | `AllSkipped`

`AllSkipped` means no live backends were available. The brief renders it as "not measured
this build" — not 0% coverage and not a regression.

---

## Error conditions

| Condition | Exit | Message |
|---|---|---|
| History file missing / unreadable | 1 | Cannot open history file |
| Empty history | 1 | history contains no records |
| `--build-id` absent from history | 1 | build id ... absent from history (no fallback) |
| `--since-build` absent from history | 1 | --since-build absent from history |
| `--format` not `markdown` | 1 | only --format markdown is supported in V1 |

---

## Design notes

- **Offline, no credentials** — reads JSONL inputs only; no live cluster access (NFR1/NFR2).
- **Deterministic** — given the same inputs and `--build-id`, output is byte-identical (NFR1).
- **Schema-faithful** — status/delta vocabulary is verbatim from the tracker; no new names invented (NFR3).
- **No aliases** — build id + version + cluster hostname are always named explicitly; "latest",
  "the cluster", "nonprod", "staging" are banned (FR2).
- **Backend pairs are data-driven** — the brief enumerates whatever pairs appear in history;
  DAX↔SQL is the only pair today (MDX deferred per vision OQ#1).

---

## Development

```sh
# Tests (all 19 ACs covered via fixtures in src/tests/fixtures/):
bash ~/.claude/skills/cloudbuild/cloudbuild.sh test mqo-parity-brief --

# Release build:
bash ~/.claude/skills/cloudbuild/cloudbuild.sh release mqo-parity-brief --
```

---

## PRD

`/Users/jsy/Documents/PRDs/PRD-mqo-parity-brief.md`
Vision: `visions/cross-backend-parity-coverage.md` — end-state #3, Component #3.
Upstream contract: `mqo-parity-coverage-tracker` (JSONL history store + coverage/delta output).
