# mqo-dax-compiler

## Recent

- v0.3.0: Member filter grounding — `Filter::Member` now resolves hierarchy to real level-qualified column via `DaxCatalogContext`, or emits typed `DaxCompileError::UngroundedMemberFilter` instead of broken `Hierarchy[Hierarchy]` (closes 70+/400 TPC-DS rollout errors).

DAX is the default compile target for the MQO: `EVALUATE` returns a **flat table**
that is compact and 1:1 with JSON array-of-objects (LLM-native), while preserving
multidimensional semantics (measures, relationships, time-intelligence). This CLI
takes a `BoundMqo` and emits syntactically valid DAX.

Part of the MQO fleet (deps: `mqo-spec`, `mqo-catalog-binder`). Consumes a
`BoundMqo` JSON produced by `mqo-bind`, emits a DAX `EVALUATE` query string.

## Why DAX

- `EVALUATE` returns a **flat table** — JSON array-of-objects — directly LLM-native.
  MDX returns a multidimensional cellset that produces cross-product blow-up
  (the `rowLimitAdvisory` in `mcp-server` proves it burns inference credits).
- AtScale's `DaxEvaluateHandler` returns tabular results natively; no reshape step.
- Rides the SSDAX + Power BI investment in the AtScale charter.
- **Execution path (confirmed 2026-06-10):** compiled `EVALUATE` text is sent to
  `/v1/xmla` via `mqo-auth-bridge` — not PGWire. PGWire (`:15432`) is SQL-only;
  DAX `EVALUATE` is rejected at the wire level. `/v1/xmla` requires `<Cube>` in
  the XMLA `<PropertyList>` when the statement is DAX.

## Usage

```bash
mqo-dax --bound <bound_mqo.json>
```

Stdout: DAX `EVALUATE` text. Exit codes: 0 success, 1 compile error, 2 I/O error.

### Options

| Flag | Description |
|------|-------------|
| `--bound <PATH>` | Path to `BoundMqo` JSON produced by `mqo-bind` |
| `--skip-syntax-check` | Skip bundled structural DAX syntax check (not recommended) |

## Acceptance criteria

1. At least 8 golden `BoundMqo` → DAX pairs compile correctly.
2. Each `TimeIntel` variant maps to the correct DAX function (`YoY`→`SAMEPERIODLASTYEAR`, `PriorPeriod`→`DATEADD`, `ToDate{Year}`→`DATESYTD`, `ToDate{Quarter}`→`DATESQTD`, `ToDate{Month}`→`DATESMTD`, `RunningTotal`→`DATESINTORANGE`, `Share`→`DIVIDE+ALL`, `Rank`→`TOPN`).
3. Calc-group members are emitted as column filters (`CalcGroup[CalcGroup] = "member"`), not invented time-intelligence logic.
4. `limit` produces `TOPN`; `order` produces `ORDER BY` with correct direction.
5. All emitted DAX passes the bundled structural syntax check (balanced parens/brackets/quotes, recognized table construct). No engine round-trip required.
6. `cargo test --release` passes; clippy clean.

## Install

```bash
cargo install --path .
```

Requires: Rust 1.70+, `mqo-spec` as a sibling path dep (`../mqo-spec`).

## Test suite

```bash
cargo test          # 57 tests: 20 unit + 33 acceptance + 4 integration CLI
cargo test --release
cargo clippy --workspace -- -D warnings
cargo deny check licenses bans
```

Mutation kill rate: **96.1%** (73/76 viable mutants caught, Phase 1 telemetry).

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.
