# mcp-hypothesis-engine

**Gen-9** — Autonomous hypothesis generator for metric movements using the concept graph.

Fuses `mcp-causal-tracer` (structural derivation paths) and `mcp-next-query-proposer` (probe MQOs) into one autonomous step. Given a concept graph and two DatasetSummary handles (baseline + current), it produces a ranked `HypothesisSet` where each hypothesis pairs a structural explanation with a concrete investigative MQO.

## Usage

```
mcp-hypothesis-engine \
  --graph     concept-graph.json     \
  --target    "Total Store Sales"    \
  --handle-a  summary-baseline.json  \
  --handle-b  summary-current.json   \
  [--from-event watch-event.json]    \
  [--max-depth 4]                    \
  [--top-k 8]                        \
  [--format json|human]
```

`--target` and `--from-event` are mutually exclusive. `--from-event` accepts a `WatchEvent` JSON with `measure`/`observed`/`prior` fields (or `query.measures[0].unique_name`).

## Output

```json
{
  "target": "Total Store Sales",
  "target_delta_fraction": -0.078,
  "evidence_type": "structural",
  "analysis_note": "Hypotheses are structural derivation paths with probe queries. Statistical causation requires additional analysis.",
  "hypotheses": [
    {
      "rank": 1,
      "explanation": "Total Store Sales fell because component Store Sales Amount fell 7.5%",
      "path": ["Total Store Sales", "Store Sales Amount"],
      "path_edge_kinds": ["DerivesFrom"],
      "corroboration": "corroborated",
      "component_delta_fraction": -0.075,
      "probe_mqo": { "measures": [{"unique_name": "Store Sales Amount"}], "dimensions": [], "filters": [] },
      "confidence": "high"
    }
  ]
}
```

Every output carries `evidence_type: "structural"` and the verbatim `analysis_note` as honesty guardrails.

## Algorithm

1. Resolve target from `--target` or `--from-event`
2. BFS over outgoing `DerivesFrom`/`AggregatesVia`/`FiltersBy` edges up to `--max-depth`
3. For each candidate leaf, compute component delta from handle summaries
4. Classify as `corroborated` (same direction as target delta) or `structural_only`
5. Synthesize probe MQO: `{measures:[{unique_name: <component>}], dimensions:[], filters:[]}`
6. Rank: corroborated first, larger |delta|, shorter path
7. Emit top-K

**Confidence:** `high` = corroborated + depth ≤ 2; `medium` = corroborated deeper or structural_only shallow; `low` otherwise.

## Acceptance Criteria

| # | Criterion | Test |
|---|-----------|------|
| AC1 | Corroborated probe has correct path, corroboration, and measure | `tests/ac1_corroborated_probe.rs` |
| AC2 | All probe_mqos are structurally valid | `tests/ac2_probe_valid.rs` |
| AC3 | `evidence_type` and `analysis_note` always present | `tests/ac3_evidence.rs` |
| AC4 | No-data components yield `structural_only` ranked below corroborated | `tests/ac4_structural_only.rs` |
| AC5 | `--from-event` resolves target + delta | `tests/ac5_from_event.rs` |
| AC6 | Missing target → empty hypotheses; `--top-k` limits output | `tests/ac6_missing_and_topk.rs` |
| AC7 | 500-node graph, depth-4, top-8 completes < 250ms | `tests/bench_perf.rs` |

## Dependencies

- `mcp-concept-graph` (path dep) — attributed property graph of the semantic model
- `serde`, `serde_json` — serialization
- `clap` — CLI argument parsing

No network, no LLM.

## Non-goals

- Executing probe MQOs (Gen-9.1, pending `mcp-query-budget-governor`)
- Statistical causal inference
- Ranking by business value
