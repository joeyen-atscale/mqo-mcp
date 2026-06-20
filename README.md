# mqo-mcp

**Make the semantic layer the interface for AI analytics — a typed query object the model selects from, not SQL it writes.**

A Rust workspace (~50 crates) built on one idea: against a governed multidimensional
model, the dangerous failures are *silent*. A model that writes its own SQL can return a
coherent answer to the wrong question — the wrong measure, the wrong date role, an
incompatible hierarchy path — and nothing about the result looks wrong. The remedy is to
stop asking the model to write SQL and instead give it a closed query grammar it can only
*select* from, ground every field against the live model before execution, and keep the
result rows on the server so the model orchestrates the analysis but is never the
calculator.

That contract has three parts, and the workspace is organized around them:

1. **A closed query object — the MQO.** Selection-only, enumerable, validated. The model
   picks measures and levels; it cannot express a free-form join or a hand-written filter.
2. **Deterministic grounding and guards.** Every reference is resolved to an exact
   `unique_name` against a live catalog snapshot, and an always-on validator rejects the
   coherent-but-wrong query before it reaches the engine.
3. **A handle protocol.** Results stay server-side behind a handle; follow-up operations
   run as DuckDB SQL over the persisted table, so raw rows never cross back into the model's
   context.

It powers the [`mqo-demo`](https://github.com/joeyen-atscale/mqo-demo) chat app.

## The pipeline

```
query_multidimensional(MQO)         ← a selection-only typed object, never raw SQL
  → param-validate (always-on grounding gate; rejects coherent-but-wrong before execution)
  → bind   (resolve every reference to an exact unique_name against the catalog)
  → route  (DAX / MDX / SQL, chosen by query shape and cardinality)
  → compile → execute  (DAX/MDX over XMLA, SQL over PGWire)
  → store result as a handle; profile → recommend → emit a Vega-Lite chart
```

The model issues one `query_multidimensional` call per question. Everything after — slice,
aggregate, period-over-period, chart — runs against the handle, so a 10,000-row result and a
50-row result cost the same in context after the first page.

## The crate families (50 members)

| Prefix | Count | What it is |
|---|---|---|
| `mqo-*` | 29 | the pipeline — spec, catalog-binder, backend-router, DAX/MDX compilers, auth-bridge, result-profiler, chart toolkit, param-validator, benches, parity tracking |
| `dh-*` | 7 | the dataset-handle layer — spec, store, op kernel, summarizer, export; keeps result rows off the model |
| `mcp-*` | 6 | federation and the active-investigation surface — cluster registry/health/diff, interaction scorer, investigation orchestrator, spike-evidence bundle |
| `mqoguard-*` | 5 | the guard suite — column-group enrichment, compatibility matrix, filter-bind report, null-path detector, regression harness |
| `aso-*` | 3 | the BFO-grounded ontology layer — OWL2-DL vocabulary, engine-model → RDF lift, kind-driven grounding overlay |

The capstone binary is **`mqo-mcp-server`**: a stdio MCP server exposing 28 read-only tools
(`query_multidimensional`, the catalog tools, the `dataset_*` handle ops, the chart and
federation tools). Four pipeline stages — `mqo-bind`, `mqo-route`, `mqo-dax`, `mqo-mdx` — are
standalone binaries the server composes by passing JSON between them, so each is testable in
isolation and callable from a non-Rust client.

[`ARCHITECTURE.md`](./ARCHITECTURE.md) is the place to start reading: it covers the
linked-vs-subprocess boundary, the handle protocol, the param-validator firewall, and the
key design decisions with their rationale.

## Build

The toolchain is pinned (Rust 1.88.0, see `rust-toolchain.toml`); the workspace forbids
`unsafe`, runs strict clippy, and gates licenses with `cargo-deny`.

**Just the engine binaries** the demo needs (what `mqo-demo`'s `install.sh` builds):

```bash
cargo build --release \
  -p mqo-mcp-server -p mqo-catalog-binder \
  -p mqo-backend-router -p mqo-dax-compiler -p mqo-mdx-compiler
# → target/release/{mqo-mcp-server, mqo-bind, mqo-route, mqo-dax, mqo-mdx}
```

**Full workspace** (engine development):

```bash
cargo build --workspace      # or: cargo test --workspace
```

`mqo-mdx-compiler` vendors its own copy of `mqo-spec` for CI isolation and is excluded from
the workspace — build and test it standalone if you touch it.

## Run

The server runs cluster-free out of the box. A recorded TPC-DS catalog snapshot ships at
`mqo-mcp-server/fixtures/`, and without `--endpoint` the server answers against a fixture
engine that synthesizes bounded rows — enough to exercise the full pipeline and the handle
ops with no cluster.

```bash
mqo-mcp-server --catalog mqo-mcp-server/fixtures/tpcds_catalog.json
```

Transport is MCP JSON-RPC 2.0 over stdio: newline-delimited JSON in, newline-delimited JSON
out. The server resolves the four pipeline binaries from `--release-dir`, then `~/.local/bin`,
then `PATH`.

To connect to a live AtScale cluster, add `--endpoint` and the OIDC flags. XMLA (DAX/MDX)
goes to `--xmla-url`; SQL goes to PGWire on the endpoint host. Secrets are referenced by
**env-var name only** (`--oidc-client-secret-env ATSCALE_CLIENT_SECRET`, and similar for the
PGWire credentials) — no secret or connection string is ever written to config. See
[`mqo-mcp-server/README.md`](./mqo-mcp-server/README.md) for the full live-mode invocation,
including the AtScale Cloud and Community-Edition variants.

## How it works — the parts worth knowing

- **The MQO is selection-only.** Read-only by construction: there is no write path, so the
  "can this tool mutate data?" question disappears. The catalog tools advertise
  `readOnlyHint: true`.
- **The param-validator is always on.** It runs inside the server on every
  `query_multidimensional` call, before the binder, and rejects measures, levels, or
  cross-subject-area paths that aren't in the catalog snapshot — each with a nearest-match
  suggestion (Jaro-Winkler) the model is expected to apply and retry. This is server-side
  enforcement of what prompt rules used to attempt.
- **Handles are the currency.** `query_multidimensional` persists results and returns a
  handle (or inline rows when the result is small); every follow-up op derives a *new*
  handle, so chains are immutable and reproducible. Handles are per-process and TTL-evicted;
  they do not survive a server restart.

## Where it fits

This is the MQO/MCP corner of the wider AtScale fleet. The sibling
[`mqo-demo`](https://github.com/joeyen-atscale/mqo-demo) is the chat application that drives
this server end-to-end. Within the workspace, the `dh-*` cluster is the longer-term direction:
`dh-mcp-server` links `mqo-mcp-server` as a library and adds the full `dh-ops` kernel, and is
intended to subsume it once the complete op set and cross-session handle durability are
proven.

## Status

The pipeline, the validator, the handle store, and the MCP server are working and tested;
the fixture engine makes the whole stack runnable without a cluster. Several crates are
benches and harnesses rather than runtime components — `mqo-paramq-bench` and
`mqo-vs-sql-bench` measure validator and end-to-end accuracy offline, and the `mcp-*`
active-investigation crates (hypothesis engine, investigation orchestrator) are earlier-stage
than the core pipeline. Accuracy numbers belong with the runs that produced them, not in this
README; run the benches to reproduce them.

Dual-licensed **MIT OR Apache-2.0**.
