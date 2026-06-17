# aso-ground Changelog

## v0.1.0 — 2026-06-16

**PRD:** PRD-osl-bfo-grounding — deterministic kind-driven BFO 2020 grounding overlay
over the `aso-lift` RDF graph.

### Summary

Fresh `mqo-mcp` workspace crate implementing the PRD's grounding intent:

- Reads a lifted Turtle graph (produced by `aso-lift`)
- Assigns each `owl:NamedIndividual` a BFO 2020 category deterministically by its `aso:` class kind (no substring name heuristics)
- Emits a **non-mutating overlay** graph: each individual carries both its `aso:` class assertion and its BFO class assertion, plus grounding metadata annotations
- Supports `aso:bfoHint` literal override (wins over kind; unrecognized hints error loudly naming the element)
- `report()` function / `report` CLI subcommand emitting coverage % + breakdown of kind/hint/fallback
- Deterministic, byte-identical output on identical input (sorted triple emission)
- Offline only — no network or warehouse access

### Kind → BFO mapping

| aso: class                         | BFO 2020 category                    |
|------------------------------------|--------------------------------------|
| Measure / FullyAdditive / SemiAdditive / CalculatedMember | Generically Dependent Continuant (BFO_0000031) |
| Key                                | Quality (BFO_0000019)                |
| Dimension / Hierarchy / Level / RolePlayingReference | Role (BFO_0000023) |
| Cube / DataSet / Perspective / Attribute | Generically Dependent Continuant (BFO_0000031) |
| Unknown / unrecognized             | Independent Continuant (BFO_0000004) — fallback, counted |

### Retarget deviation from PRD

The PRD was framed as "evolve the standalone `j0yen/ousia-atscale` crate." This build
**ignored that crate entirely** and instead created a fresh crate directly in the
`mqo-mcp` workspace, stacked on the `aso-tbox` and `aso-lift` crates already present
on branch `build/osl-engine-xml-rdf-lift`. The `ousia-atscale` repo was not cloned,
fetched, depended on, or modified. PRD FR1 (fix the v0.3.0 ousia-atscale build
regression) is N/A — we are not using that crate.

### AC coverage

| AC | Status | Notes |
|----|--------|-------|
| AC1 | ✅ | `cargo test --release -p aso-ground` — 13 tests, all green |
| AC2 | ✅ | `gross_throughput` (no revenue/sales token) → GDC by kind, not fallback |
| AC3 | ✅ | `bfo_hint` wins; `"rolle"` typo → `InvalidBfoHint` naming element |
| AC4 | ✅ | Overlay: each individual has `aso:` class + BFO class assertion |
| AC5 | ✅ | Input string not mutated (test verifies checksum unchanged) |
| AC6 | ✅ | Byte-identical on two runs of identical input |
| AC7 | ✅ | Unknown `aso:` kind → `IndependentContinuant`, counted in `report` |
| AC8 | ⏭ | Stub/skip — describe_model JSON transition mode not implemented in v0.1 (MAY requirement) |
