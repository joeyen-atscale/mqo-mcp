# Architecture — MQO/MCP workspace

This workspace is the Rust implementation of the AtScale MQO (Multidimensional Query
Object) pipeline and its MCP (Model Context Protocol) server. It replaces the
free-text SQL approach in the incumbent Go MCP server with a typed, catalog-bound
query object that the LLM constructs and the server compiles, routes, and executes.

---

## The core problem this solves

The incumbent server asks the LLM to write SQL and submit it via `run_query`.
Free SQL over a multidimensional model is error-prone: wrong hierarchy levels,
incompatible measure/dimension paths, miscounted date roles. Every error is a
round-trip that burned warehouse credits before failing.

The MQO replaces the open SQL grammar with a closed, enumerable object:

```json
{
  "model": "tpcds_Snowflake",
  "measures": [{"unique_name": "Total Store Sales"}],
  "dimensions": [{"level_unique_name": "Calendar Month"}],
  "filters": [],
  "limit": 200
}
```

Every field is validated against the live catalog before compilation. An invalid
`unique_name` is caught by the param-validator before any query reaches the engine.

---

## Crate roles and integration boundaries

There are two fundamentally different ways crates in this workspace integrate:

### 1. Linked at compile time (path deps in Cargo.toml)

These crates are imported by `mqo-mcp-server` and compiled into the same binary:

```
mqo-mcp-server
    ├── mqo-spec              (MQO types, JSON Schema validation)
    ├── mqo-auth-bridge       (Engine trait, LiveExecutor, FixtureEngine)
    ├── mqo-duckdb-handle-store (ResultStore: MemStore + DuckStore)
    ├── mqo-result-profiler   (EngineResult → ResultProfile)
    ├── mqo-chart-recommender (ResultProfile → ChartRecommendation)
    ├── mqo-vega-emitter      (recommendation + data → Vega-Lite v5 spec)
    ├── mcp-cluster-registry  (clusters.toml federation)
    ├── mcp-cluster-health-monitor (per-cluster backend probes)
    └── mcp-cross-cluster-diff (diff across cluster model surfaces)
```

**Why linked:** these are on the hot path of every tool call. Linking avoids
serialization overhead and lets the Rust type system enforce invariants across
the boundary (e.g., the `Engine` trait, `ResultStore` trait).

### 2. Subprocess tools (binaries on $PATH, called via `ToolPaths`)

These run as separate processes, communicate via stdin/stdout JSON, and are
**not** path deps of `mqo-mcp-server`:

```
mqo-catalog-binder   MQO JSON → BoundMqo JSON
mqo-dax-compiler     BoundMqo JSON → DAX EVALUATE text
mqo-mdx-compiler     BoundMqo JSON → MDX SELECT text  (*)
mqo-backend-router   BoundMqo + CatalogStats → routing decision JSON
```

(*) `mqo-mdx-compiler` vendors `mqo-spec` for CI isolation and is excluded from
this workspace. It is the only crate with this design — all others use the
workspace copy.

**Why subprocess:** these tools are independently deployable, testable without
the server, and can be updated without recompiling the server. The compose-by-JSON
pattern also means a Python or Go client can call them. The server assembles the
pipeline by calling each tool in sequence:

```
pipeline.run(mqo)
  → binder(mqo + catalog) → BoundMqo
  → router(BoundMqo + stats) → {backend, estimated_rows, sql_projection}
  → compiler(BoundMqo + decision) → compiled_query
  → engine.execute(compiled_query, backend) → EngineResult
```

The swap point for the engine is `pipeline.rs:222`. In fixture mode (CI, no
cluster), a `FixtureEngine` returns synthetic rows. In live mode, `LiveExecutor`
uses `mqo-auth-bridge` to fetch an OIDC token and issue the query over PGWire
or XMLA.

---

## Handle-based result protocol

After the initial `query_multidimensional` call, the LLM **never re-queries
AtScale** for follow-up operations. Instead:

1. `query_multidimensional` persists results in `mqo-duckdb-handle-store` and
   returns a handle UUID (or inline rows if `row_count ≤ PAGE_SIZE`).
2. All follow-up operations use handle ops — server-side DuckDB SQL over the
   persisted table:

```
H0 = query_multidimensional(mqo)              ← one AtScale round-trip
H1 = dataset_slice(H0, {state = "CA"})        ← DuckDB WHERE
H2 = dataset_aggregate(H1, group=[Month])     ← DuckDB GROUP BY
H3 = dataset_period_over_period(H2, ...)      ← DuckDB LAG window
     dataset_chart(H3, "bar", ...)            ← Vega-Lite spec from ≤20 rows
```

**Why this matters for token cost:** raw rows crossing the MCP boundary into the
LLM context are the largest token class. With handles, `tool_result_rows` in the
session footprint flatlines at `K × schema_bytes` (K = INLINE_THRESHOLD = 20)
regardless of how many rows the query returned. A 10,000-row result and a 50-row
result cost the same in context after the first page.

For large results (> PAGE_SIZE = 50 rows), the cursor protocol applies:
`query_multidimensional` returns `{cursor_id, page, has_more}`. `next_page` pages
through it. Handle ops accept a `cursor_id` wherever a handle is expected.

**Immutable semantics:** every handle op derives a new handle. The input handle
is never mutated. This preserves lineage and makes the chain reproducible.

**TTL eviction:** handles expire after `--cursor-ttl-secs` (default 600s). A
`CursorExpired` error means re-run the initial query. The TTL is per-server
process; handles do not survive a server restart.

---

## The param-validator firewall

`mqo-param-validator` runs inside the server on every `query_multidimensional`
call, before the MQO reaches the binder. It rejects:
- Measures not in the `CatalogSnapshot`
- Dimension levels not in the `CatalogSnapshot`
- Incompatible subject-area paths (measure + dimension from different subject areas)

Each rejection includes a `nearest_match` suggestion (Jaro-Winkler via `strsim`).
The LLM is expected to fix the rejected field to the suggestion before retrying.
This is the server-side enforcement of what R1–R13 in the legacy `query-semantic-layer`
skill tried to enforce via prompt rules.

`mqo-paramq-bench` measures this offline against `tpcds_failure_modes_100`: five
failure modes, ground truth `{canonical, rejected}` pairs, scored for first-try
valid rate and pass@k.

---

## Observability: three-ring model

```
Ring 1 — Server runtime (always on)
  mqo-mcp-server process

Ring 2 — Continuous measurement (run after changes, not linked to server)
  mqo-session-footprint-meter   token class profiler (launches server as child process)
  mqo-paramq-bench              offline validator quality bench
  mqo-handle-walkthrough        scripted 4-turn POC asserting requery_count == 1

Ring 3 — Quality audit (run before grooming)
  trajectory-audit              flags LLM-as-calculator, ignored rejections, handle leakage
  mcp-spike-evidence-bundle     maps ATSCALE-49212..49215 ACs to artifact verdicts
```

Ring 2 and Ring 3 tools are external harnesses — they observe the server via
stdio/JSON-RPC, not via linked code. They're in the workspace for consistent
`cargo test --workspace` and shared `[profile.release]` settings.

---

## The dh-* cluster

`dh-mcp-server` is a successor server that links `mqo-mcp-server` as a library
and adds the full `dh-ops` kernel (aggregate/filter/sort/top_n/pivot/compare/drill).
The `dataset_*` tools in `mqo-mcp-server` v0.6.x are a minimal subset of the
`dh-ops` surface — promoted from the handle walkthrough proof-of-concept.

The longer-term direction is `dh-mcp-server` subsuming `mqo-mcp-server`, once
the full op set and cross-session handle durability are proven.

---

## Key design decisions

| Decision | Choice | Why |
|---|---|---|
| Query language | DAX primary, SQL fallback | DAX returns flat tables (LLM-native); SQL on clusters without SSDAX |
| Result backend | DuckDB (feature-gated) | Candidate from ATSCALE-49212; measured before Arrow/hand-rolled |
| Pipeline | Subprocess composition | Independent deployment; language-agnostic clients; testable in isolation |
| Handle store | In-process per server | v1 simplicity; multi-client sharing deferred to dh-store daemon |
| Inline threshold | K=20 | Head-sample in tool result stays small; full result is server-side |
| Page size | 50 rows | Balances context cost vs. usability for tabular preview |
| Auth | OIDC ROPC for XMLA; direct creds or OIDC for PGWire | AtScale MDX requires bearer token; SQL path accepts both |
| `count_distinct` | HashSet\<String\> over column values | DuckDB HLL for large sets; for K=20 head-sample sizes, exact is fine |
| period_over_period | Wide format | LLM reads one row per period; Vega-Lite `y_cols` takes both `Sales` and `Sales_prior` directly |

---

## Running the workspace

```bash
# Check all crates compile
cd ~/Documents/projects
cargo check --workspace

# Run all tests (excludes mqo-mdx-compiler — run that separately)
cargo test --workspace --release

# Build the capstone server
cargo build -p mqo-mcp-server --release --features duckdb

# Run a specific crate's tests
cargo test -p mqo-param-validator --release
cargo test -p mqo-duckdb-handle-store --release

# Check for dependency version drift
cargo tree --workspace --duplicates
```

`mqo-mdx-compiler` is excluded from this workspace (see `exclude` in Cargo.toml).
Run it standalone:
```bash
cd ~/Documents/projects/mqo-mdx-compiler
cargo test --release
```

---

## What's NOT here

- **`mqo-from-sql`** (also at `~/builds/mqo-from-sql`) — reverse-compiles AtScale
  SQL to MQO; included in the workspace but has no path deps from the server.
- **`*-port` crates** — wintermute agent integration ports; different concern entirely.
- **`slai-*`, `recall-*`, `provfs-*`** — unrelated to the MQO pipeline.
- **`mqo-mdx-compiler`** — excluded (vendored dep conflict); build standalone.
