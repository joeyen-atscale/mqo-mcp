# PRD-mqo-vs-sql-bench — prove multidimensional > tabular (MQO vs text-to-SQL)

- Status: Draft v0.1
- build_target: rust-cli
- Vision: visions/multidimensional-query-object.md
- Depends on: mqo-mcp-server; reuses slai-text-to-sql-accuracy-bench graders (shell-out)
- Owner: Joe Yen
- Date: 2026-06-07

## TL;DR

The vision's hypothesis — *multidimensional datastores beat tabular for analytics,
and especially as an AI grounding layer* — must be a number, not a slogan. This CLI
runs a golden NL-question set through two arms — (A) the text-to-SQL `run_query`
path and (B) the MQO `query_multidimensional` path — and emits a head-to-head report
on accuracy, invalid-entity (hallucination) rate, retries, latency, and tokens.

## Why this exists

`visions/semantic-layer-for-ai.md` cites ~20% (raw text-to-SQL) → ~95% (governed
semantic layer) per Self-Assessment Dossier PTC/4713087073, and the `slai-*` fleet
builds the graders. None of those test whether the **object interface** itself beats
the SQL interface. This benchmark closes that loop and produces the evidence the
vision is built to generate.

## What this builds

A Rust CLI `mqo-bench`:

- Input: `--tasks <tasks.json>` — NL questions, each with an expected result (or a
  reference query) and the target model.
- For each task: run arm A (model authors SQL → run_query) and arm B (model builds
  an MQO → query_multidimensional); capture result, retries, latency, token counts.
- Grade result equivalence by shelling out to an existing grader (e.g.
  `sql-structural-diff` / a result-set comparator from the slai/semantic-grader
  fleet); count invalid-entity errors per arm.
- Output: per-question rows + an aggregate report (JSON + markdown) with per-metric
  deltas and a named winner per metric.

Deps: `serde_json`, `clap`; graders invoked as published CLIs (compose-by-JSON).

## Acceptance criteria

- AC1: Runs both arms over a tasks file and produces a per-question result table.
- AC2: Emits aggregate accuracy, invalid-entity rate, retry count, latency, and token
  deltas between the two arms.
- AC3: Result equivalence is judged by an external grader (shell-out), not bespoke
  string compare; the grader command is configurable.
- AC4: The report names the winning arm per metric and writes both JSON and markdown.
- AC5: Runs green end-to-end on a bundled fixture (recorded arm outputs; no live cluster
  or model key required for the test).
- AC6: `cargo test --release` passes; clippy clean.
