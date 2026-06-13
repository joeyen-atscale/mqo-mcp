# mqo-mcp

**The AtScale MQO/MCP fleet — making the semantic layer the interface for AI analytics.**

A Rust workspace (~50 crates) that replaces the fragile "let the model write SQL" pattern with a
**typed, catalog-validated query object** the model selects from, grounds every field against the
live model before execution, and keeps result rows on the server behind **handles** so the model
orchestrates analysis but is never the calculator. It powers the
[`mqo-demo`](https://github.com/joeyen-atscale/mqo-demo) chat app.

The thesis: against a governed multidimensional model, the dangerous failures are *silent* —
a coherent query that answers a different question (wrong measure, wrong date role, incompatible
path). The remedy is to make the semantic layer the contract: a closed query grammar, deterministic
grounding/guards, and a handle protocol.

## The pipeline

```
query_multidimensional(MQO)         ← never raw SQL; a selection-only typed object
  → param-validate (always-on grounding gate; rejects coherent-but-wrong before execution)
  → bind   (resolve every ref to an exact unique_name)
  → route  (DAX / MDX / SQL by shape + cardinality)
  → compile→ execute (DAX/MDX over XMLA, SQL over PGWire)
  → store result as a handle; profile → recommend → emit a Vega-Lite chart
```

## Crate families (~50 members)

| Prefix | Count | What |
|---|---|---|
| `mqo-*` | 28 | the pipeline (spec, catalog-binder, backend-router, dax/mdx compilers, auth-bridge, result-profiler, chart toolkit, param-validator, benches, parity) |
| `mcp-*` | 12 | the Active Semantic Layer — concept graph, hypothesis engine, budget governor, investigation orchestrator, finding store, federation (cluster registry/health/diff) |
| `dh-*` | 7 | the dataset-handle layer (spec, store, ops, summary, export) — keeps result rows off the model |
| `mqoguard-*` | 5 | the guard suite (column-group enrichment, compatibility matrix, filter-bind report, null-path detector, regression harness) |

The capstone binary is **`mqo-mcp-server`** (stdio MCP server, 23 read-only tools). The four
pipeline tools (`mqo-bind`, `mqo-route`, `mqo-dax`, `mqo-mdx`) are standalone binaries composed by
JSON. See [`ARCHITECTURE.md`](./ARCHITECTURE.md) for the full design.

## Build

**Just the demo's engine binaries** (what `mqo-demo`'s `install.sh` does):

```bash
cargo build --release -p mqo-mcp-server -p mqo-catalog-binder \
  -p mqo-backend-router -p mqo-dax-compiler -p mqo-mdx-compiler
# → target/release/{mqo-mcp-server, mqo-bind, mqo-route, mqo-dax, mqo-mdx}
```

**Full workspace** (for engine development):

```bash
cargo build --workspace        # or: cargo test --workspace
```

Pinned toolchain, `#![forbid(unsafe_code)]`, strict clippy, `cargo-deny` license gate. A recorded
TPC-DS catalog snapshot ships at `mqo-mcp-server/fixtures/tpcds_catalog.json` for offline/grounding.

## Evidence

On a 100-question TPC-DS "failure-mode" corpus scored for governed-path correctness, executed live
against an AtScale cluster, the grounded stack reaches **95% on the strict pass^4 metric** (right on
all four rollouts) — up from a **61% pre-grounding baseline** — with the hardest wrong-hierarchy
category rising from 30% toward ceiling. *Honest caveat:* the lift confounds a grounding-coached
prompt with the server changes, and there is no controlled head-to-head against text-to-SQL yet;
see the design paper for the full evaluation and limitations.

## Notes for users

- The server connects to an AtScale cluster (XMLA over `/v1/xmla`, SQL over PGWire) with OIDC; it
  also runs cluster-free against the fixture catalog. Secrets are referenced by **env-var name
  only**, never stored in config.
- Read-only by construction: the MQO is a selection object; there is no write path.

Dual-licensed **MIT OR Apache-2.0**.
