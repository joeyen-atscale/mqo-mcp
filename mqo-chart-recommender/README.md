# mqo-chart-recommender

Chart type + encoding recommendation from `result-profile.v1` for the bi-toolkit.

## TL;DR

Given a typed column inventory — the `result-profile.v1` JSON from
`mqo-result-profiler` — this crate answers: *what chart should an LLM build?*

It is the **decision brain** of the bi-toolkit vision: a pure, deterministic step
that maps `(measure count, dimension count, dimension temporality, cardinality)` to
a Vega-Lite mark and an encoding, with a one-sentence rationale and ranked
alternatives. It holds no data and renders nothing. Input: `result-profile.v1`.
Output: `chart-recommendation.v1` JSON (`{mark, encoding, rationale, alternatives}`).
The downstream Vega emitter consumes that recommendation plus the rows to produce a
spec; this crate just decides the shape. Pure Rust + JSON, no network,
macOS-trivial.

## Acceptance criteria

| AC | Level | Description |
|----|-------|-------------|
| AC1 | MUST | 1 measure + 1 temporal dim → `mark = Line`, `x` = temporal field, `y` = measure field. |
| AC2 | MUST | 1 measure + 1 nominal dim (low cardinality) → `mark = Bar`. |
| AC3 | MUST | 2 measures, 0 dims → `mark = Point` (scatter), `x` and `y` being the two measures. |
| AC4 | MUST | 1 measure, 0 dims → `mark = BigNumber`. |
| AC5 | MUST | High-cardinality nominal dim (> 25) still returns `mark = Bar` but `alternatives` contains `Table` with a cardinality rationale. |
| AC6 | MUST | 0-measure profile → `mark = Table` with a rationale explaining nothing is quantitative. |
| AC7 | MUST | Every recommendation carries a non-empty `rationale` and deterministic `alternatives` ordering. |

## Install

Add to your `Cargo.toml`:

```toml
[dependencies]
mqo-chart-recommender = { path = "../mqo-chart-recommender" }
```

Or install the binary:

```sh
cargo install --path .
```

## Usage (library)

```rust
use mqo_chart_recommender::{recommend, Mark};
use serde_json::json;

let profile = json!({
    "schema": "result-profile.v1",
    "columns": [
        {"name": "order_date", "role": "dimension", "is_temporal": true, "cardinality": 365},
        {"name": "revenue",    "role": "measure",   "is_temporal": false, "cardinality": null}
    ]
});
let rec = recommend(&profile).unwrap();
assert_eq!(rec.mark, Mark::Line);
println!("{}", serde_json::to_string_pretty(&rec).unwrap());
```

## Usage (binary)

```sh
# JSON output (default)
mqo-chart-recommender --profile profile.json

# Human-readable output
mqo-chart-recommender --profile profile.json --format human
```

## Selection rules

| Measures | Dimensions | Condition | Mark |
|----------|-----------|-----------|------|
| 1 | 0 | — | `BigNumber` |
| 1 | 1 | temporal dim | `Line` |
| 1 | 1 | nominal dim, cardinality ≤ 25 | `Bar` |
| 1 | 1 | nominal dim, cardinality > 25 | `Bar` + `Table` alternative |
| 2 | 0 | — | `Point` (scatter) |
| 2 | 1 | — | `Point` coloured by dim |
| 1 | 2 | one temporal | `Line` (multi-series) |
| 1 | 2 | both nominal | grouped `Bar` + `Rect` alternative |
| 0 | * | — | `Table` |
| * | * | unmatched | `Table` (fallback) |

## Non-goals

- Rendering or Vega-Lite spec emission (that is `mqo-vega-emitter`)
- Reading rows or the catalog directly — only `result-profile.v1` consumed
- `theta`/pie encodings beyond leaving the channel `None` in v1
- Network access of any kind

## License

MIT OR Apache-2.0
