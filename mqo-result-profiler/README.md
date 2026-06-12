# mqo-result-profiler

Typed column inventory from a `query_multidimensional` response + catalog JSON, for downstream charting.

## TL;DR

The `mqo-mcp-server` `query_multidimensional` tool returns everything a chart needs, but in a shape
an LLM has to re-derive by squinting at rows: which columns are quantitative vs categorical, which
categorical column is *time*, how many distinct values a dimension has, what range a measure spans.

This crate is the foundation of the bi-toolkit vision: it takes a `query_multidimensional` response
plus the catalog and produces a **typed column inventory** — `ResultProfile` — that names each
column's role (measure/dimension, from the `bound` MQO), its data type
(quantitative/temporal/nominal), its cardinality, null rate, and measure range, plus the semantic
flags (`is_calc`, `semi_additive`) the catalog already carries.

Everything downstream — the chart recommender, the Vega emitter, the asset bundler — consumes the
`result-profile.v1` JSON this crate emits. Pure Rust + JSON, no network, no async, macOS-trivial.

## Acceptance criteria

| # | Level | Description |
|---|-------|-------------|
| AC1 | MUST | Bound measures → `Role::Measure`/`DataType::Quantitative`; bound dimensions → `Role::Dimension` |
| AC2 | MUST | Dimension in `time.*` catalog hierarchy → `DataType::Temporal`, even with integer row values; ISO-date string fallback when hierarchy absent |
| AC3 | MUST | Non-temporal string dimension → `DataType::Nominal`; `cardinality` = distinct non-null count |
| AC4 | MUST | Quantitative measure reports `measure_range = (min, max)` over non-null rows; `null_rate` reflects nulls |
| AC5 | MUST | Catalog `semi_additive` block → `semi_additive = true`; `is_calc = true` measure → `is_calc = true` |
| AC6 | MUST | Empty `rows` array → `row_count = 0`, columns still typed from `bound`/catalog, no panic |
| AC7 | MUST | Columns ordered by `bound` projection order; serialization stable across runs |

## Install

Add to your `Cargo.toml`:

```toml
[dependencies]
mqo-result-profiler = { git = "https://github.com/joeyen-atscale/mqo-result-profiler" }
```

Or use the binary directly:

```sh
cargo install --git https://github.com/joeyen-atscale/mqo-result-profiler
```

## Usage (library)

```rust
use mqo_result_profiler::profile;

let response = serde_json::json!({
    "rows": [{"revenue": 100.0, "year": 2021}],
    "bound": { "measures": ["revenue"], "dimensions": ["year"] }
});
let catalog = serde_json::json!({
    "columns": [
        {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
        {"unique_name": "year", "label": "Year", "kind": "dimension", "hierarchy": "time.calendar"}
    ]
});
let p = profile(&response, &catalog).unwrap();
// p.columns[0].role == Role::Measure, data_type == DataType::Quantitative
// p.columns[1].role == Role::Dimension, data_type == DataType::Temporal
```

## Usage (binary)

```sh
mqo-result-profiler --response response.json --catalog catalog.json
mqo-result-profiler --response response.json --catalog catalog.json --format human
```

## License

MIT OR Apache-2.0
