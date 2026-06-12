# mqo-backend-router

**Route a `BoundMqo` to DAX, MDX, or SQL before execution — cheapest path, no wasted warehouse credits.**

`mqo-route` estimates result cardinality from level cardinalities _before_ touching the engine, then picks the cheapest capable backend: DAX for small aggregates, MDX for multidimensional shape queries, SQL/PGWire-streaming for large extracts. By the time MDX overflows the row cap, the warehouse credits are already spent. This CLI is the proactive escape hatch.

**Execution routing (confirmed 2026-06-10):** both `dax` and `mdx` decisions are
executed via `/v1/xmla` in `mqo-auth-bridge` — not PGWire. PGWire (`:15432`) is
SQL-only; only `sql` decisions go there. `/v1/xmla` accepts `EVALUATE` (DAX) and
`SELECT … ON COLUMNS` (MDX) as the SOAP `<Statement>` body; DAX requires `<Cube>`
in `<PropertyList>`.

Part of the [MQO fleet](https://github.com/joeyen-atscale).

## Install

```bash
cargo install --path .
```

Requires the `mqo-spec` sibling crate at `../mqo-spec`.

## Usage

```bash
mqo-route --bound <bound_mqo.json> --stats <level_cardinalities.json> [--row-threshold N]
```

**Output** (stdout, JSON):
```json
{
  "backend": "dax",
  "estimated_rows": 50,
  "reason": "estimated_rows (50) is within threshold (50000)"
}
```

Or for large extracts:
```json
{
  "backend": "sql",
  "estimated_rows": 200000,
  "reason": "estimated_rows (200000) exceeds row_threshold (50000)",
  "sql_projection": "SELECT time.calendar.[Date], product.category.[Product], sales.revenue FROM sales"
}
```

**Exit codes:** `0` = routing decision emitted, `2` = I/O or parse error.

## Routing rules

1. **MDX** — if `shape_flags.asymmetric_axes`, `drill_through`, or `cellset_requested` is set.
2. **SQL** — if `estimated_rows > row_threshold` (default 50,000). Emits a flat `sql_projection`.
3. **DAX** — otherwise (default).

`estimated_rows = Π(level cardinalities) × member-filter reduction`.

## Stats file format

```json
{
  "level_cardinalities": {
    "time.calendar.[Year]": 5,
    "geo.country.[Country]": 10
  },
  "shape_flags": {
    "asymmetric_axes": false,
    "drill_through": false,
    "cellset_requested": false
  }
}
```

## Acceptance criteria

| AC | Level | Description |
|----|-------|-------------|
| AC1 | MUST | Low-cardinality aggregated MQO routes to `dax` |
| AC2 | MUST | Shape-flagged MQO (drill-through / asymmetric / cellset) routes to `mdx` |
| AC3 | MUST | High-cardinality MQO routes to `sql` with non-empty `sql_projection` |
| AC4 | MUST | `estimated_rows` = product of level cardinalities, reduced by member filters |
| AC5 | MUST | `--row-threshold` overrides the routing boundary |
| AC6 | MUST | `cargo test --release` passes; clippy clean |
| AC7 | SHOULD | Binary integration tests via `tests/integration_cli.rs` |

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
