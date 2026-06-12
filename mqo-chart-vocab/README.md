# mqo-chart-vocab — canonical chart-recommendation.v1 mark vocabulary

The `chart-recommendation.v1` JSON contract does not actually connect its two
shipped CLIs. The recommender serializes mark types in `snake_case`
(`"bar"`, `"line"`, `"big_number"`); the emitter only accepts PascalCase
(`"Bar"`, `"Line"`, `"BigNumber"`). Piping one into the other fails outright.
Three separate crates have each hand-written their own snake↔Pascal shim to
paper over the gap, and a third-party client wiring the published schema gets a
hard error with no shim to save it.

This crate pins `snake_case` as the ONE canonical wire spelling for
`chart-recommendation.v1` marks and provides a `Mark` enum, conformance
helpers, and a deprecation-window bridge so both producer and consumer can share
a single source of truth.

## Usage

```rust
use mqo_chart_vocab::{Mark, parse_mark, is_legacy_pascal, canonical_mark_str};

// Deserialize from canonical snake_case (recommender output):
let m = parse_mark("big_number").unwrap();
assert_eq!(m, Mark::BigNumber);

// Deserialize from legacy PascalCase (stored artifact during deprecation window):
let m = parse_mark("BigNumber").unwrap();
assert_eq!(m, Mark::BigNumber);
if is_legacy_pascal("BigNumber") {
    eprintln!(
        "DEPRECATION WARNING: {:?} is legacy PascalCase; use {:?} instead",
        "BigNumber",
        canonical_mark_str(&m),
    );
}

// Unknown mark → None (MalformedRecommendation):
assert_eq!(parse_mark("piechart"), None);

// Serialize back to canonical wire form:
let json = serde_json::to_string(&Mark::BigNumber).unwrap();
assert_eq!(json, r#""big_number""#);
```

## Install

```toml
mqo-chart-vocab = { path = "../mqo-chart-vocab" }
```

(Path dependency — this crate is not published to crates.io.)

## Wire spelling reference

| Rust enum variant | Canonical wire (snake_case) | Legacy wire (PascalCase) |
|---|---|---|
| `Bar`       | `bar`         | `Bar`       (**deprecated**) |
| `Line`      | `line`        | `Line`      (**deprecated**) |
| `BigNumber` | `big_number`  | `BigNumber` (**deprecated**) |
| `Point`     | `point`       | `Point`     (**deprecated**) |
| `Area`      | `area`        | `Area`      (**deprecated**) |
| `Rect`      | `rect`        | `Rect`      (**deprecated**) |
| `Table`     | `table`       | `Table`     (**deprecated**) |

## Running conformance tests

```bash
cargo test -p mqo-chart-vocab
```

All 22 unit tests + 7 doc tests must pass green. The `mark_serde_round_trip_all_variants`
and `recommender_output_is_all_canonical` tests are the primary conformance gates — they
fail CI if the recommender's serde output ever drifts from canonical.

## Shim retirement plan

This crate replaces three private shims across the toolkit:

1. `mqo-bi-asset-bundle/src/lib.rs` — `mark_to_emitter_str()` (hand-written match)
2. `mqo-mcp-server/src/chart_tools.rs:63-74` — `recommendation_to_emitter_json()` mark-patching + `snake_to_pascal()`
3. `mqo-mcp-server/src/chart_tools.rs:82-91` — `normalize_recommendation_for_emitter()`

Once `mqo-vega-emitter`'s `map_mark()` is updated to accept canonical `snake_case`
(using `parse_mark` + `is_legacy_pascal` from this crate), these shims can be retired.
See `PRD-mqo-chart-vocab-conformance.md` for the full deprecation runway.
