# mqo-vega-emitter

Vega-Lite v5 spec emitter from `chart-recommendation.v1` + rows for the bi-toolkit.

## TL;DR

Takes a `chart-recommendation.v1` JSON (mark + encoding channels) and a rows array
(straight out of `query_multidimensional`) and emits a complete, schema-valid
**Vega-Lite v5 JSON spec** with data embedded inline. No rendering — spec text only.

Refuses to emit a spec whose encoding references a field not present in the rows,
because a spec pointing at a missing column renders blank and the LLM trusts it.

This is component 3 of the `bi-toolkit-for-mcp` vision, sitting between
`mqo-chart-recommender` (component 2) and `mqo-bi-asset-bundle` (component 4).

## Acceptance Criteria

| # | Description | Test |
|---|---|---|
| AC1 | Line recommendation emits `mark:"line"`, correct field/type on x/y, `$schema` VL5 | `tests/ac1_line_spec.rs` |
| AC2 | `data.values` equals input rows verbatim — no mutation or reordering | `tests/ac2_inline_data.rs` |
| AC3 | Bar recommendation emits `mark:"bar"` with nominal field on x channel | `tests/ac3_bar_spec.rs` |
| AC4 | Encoding channel referencing absent field returns `EmitError` — does not emit broken spec | `tests/ac4_missing_field_refused.rs` |
| AC5 | `BigNumber` recommendation emits `"text"`-mark spec with measure in `encoding.text` | `tests/ac5_bignumber_spec.rs` |
| AC6 | Quantitative channel gets `aggregate:"sum"` by default; `semi_additive` channel does NOT | `tests/ac6_semi_additive_no_sum.rs` |
| AC7 | Emitted spec is deterministic and round-trips through `serde_json` without reordering | `tests/ac7_deterministic.rs` |

## Install

```toml
# Cargo.toml
[dependencies]
mqo-vega-emitter = { path = "../mqo-vega-emitter" }
```

Or as a CLI:

```sh
cargo install --path .
```

## Usage

### Library

```rust
use mqo_vega_emitter::emit;
use serde_json::json;

let rec = json!({
    "mark": "Line",
    "encoding": {
        "x": { "field": "year", "data_type": "temporal" },
        "y": { "field": "revenue", "data_type": "quantitative" }
    }
});
let rows = vec![json!({"year": "2023", "revenue": 100})];
let spec = emit(&rec, &rows)?;
// spec["$schema"] == "https://vega.github.io/schema/vega-lite/v5.json"
// spec["mark"] == "line"
```

### CLI

```sh
mqo-vega-emitter --recommendation rec.json --rows rows.json --pretty
```

## Emission Rules

- **Mark mapping**: `Line→line`, `Bar→bar`, `Point→point`, `Area→area`, `Rect→rect`,
  `BigNumber→text` (measure in `encoding.text`), `Table→text` + `_render:"table"`.
- **Aggregation**: `quantitative` channels get `aggregate:"sum"` by default.
  `semi_additive: true` suppresses the aggregate — summing a balance over time is wrong.
- **Validation**: Every encoding field must appear in at least one row. Missing field
  → `EmitError::MissingField`, not a blank-rendering spec.
- **Determinism**: Stable field order via serde struct; byte-identical across calls.

## No Dependencies on mqo-chart-recommender

Reads `chart-recommendation.v1` as `serde_json::Value`. No crate dep on the recommender.

## Non-Goals

- No rendering (no PNG/SVG/browser/vl-convert)
- No network
- No VL5 full JSON Schema validation (structural assembly only)
- No Python/JS bindings in v1
