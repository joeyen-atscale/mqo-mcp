# AUTOBUILDER_PROGRAM.md — mqo-spec

## Contract

You are the edit-agent for `mqo-spec`. Your job is to make changes to `src/` only.
You MUST NOT modify:
- `tests/acceptance_*.rs` — acceptance tests (read-only harness)
- `scripts/` — harness scripts (read-only)
- `agent/intent-card.json` — intent card (locked after Stage 1)
- `agent/proof-lanes.toml` — routing config (read-only)
- `deny.toml`, `clippy.toml`, `rust-toolchain.toml` — tooling config

## Success criteria (from intent-card.json)

All MUST ACs must be green before advancing:

- **AC1** — All MQO types round-trip through JSON losslessly for every fixture.
- **AC2** — `mqo-spec` emits a valid JSON Schema document for `Mqo`.
- **AC3** — `validate()` rejects: empty measures, limit 0, Range lo > hi — each with a distinct MqoError.
- **AC4** — At least 6 golden fixtures parse and validate.
- **AC5** — `cargo test --release` passes; `cargo clippy -- -D warnings` is clean.

SHOULD AC (bonus):
- **AC6** — `BoundMqo` carries resolved unique_names + per-ref metadata flags.

## Hard constraints

- Rust edition 2021
- `#![forbid(unsafe_code)]` must remain
- Max 8 direct dependencies
- MSRV: 1.70.0
- Allowed deps: `serde`, `serde_json`, `schemars`, `thiserror`

## Iterate-and-prove loop invariants

1. One hypothesis per iteration — state it in the commit message.
2. Run `scripts/run-metrics.sh` after each edit. Advance only if all hard gates pass AND quality_score improved AND no MUST-AC regression.
3. On crash: tail -n 50 run.log; ≤ 3 fix attempts; else write FailureCapsule and stop.
4. git commit -m "iter-N: <hypothesis>" — one commit per iteration.
