# mqo-chart-caption

Deterministic, data-computed takeaway captions for BI assets (`chart-caption.v1`).

## TL;DR

Takes a `result-profile.v1` profile (from `mqo-result-profiler`) plus result rows and emits a `chart-caption.v1` payload: a one-line headline takeaway plus 0–3 supporting facts. **Every number is computed from the data** — no LLM in the loop, fully deterministic. Identical inputs always produce byte-identical output.

## What it does

- **Trend claims** — first→last delta and % change over an ordered/temporal dimension ("Revenue rose 45.8% from $4.8M to $7.0M across 2020–2024")
- **Leader claims** — top category by a measure, with optional runner-up ("APAC leads Margin % at 24.1, ahead of NA's 21.0")
- **Extremum claims** — min/max of a measure with the holding category
- **Coverage notes** — null/missing data warnings when `null_rate` is material

Caveat guards (matching `mqo-bi-asset-bundle`):
- No summed-total claims for `semi_additive` measures over a temporal axis
- No sum/total for `is_calc` percentage measures
- No leader/extremum claims for nominal dimensions with cardinality > 25

Operator control plane:
- Inspect `provenance` — every considered claim with source columns, computed values, and whether it fired (and why not)
- Suppress any claim category per measure (or globally) via `CaptionConfig::suppressions`

## Usage

```rust
use mqo_chart_caption::{generate_caption, CaptionInput, CaptionConfig};

let caption = generate_caption(&CaptionInput { profile, rows }, &CaptionConfig::default())?;
println!("{}", caption.headline);
// "Revenue rose 45.8% from $4.8M to $7.0M across 2020–2024"
```

## `chart-caption.v1` schema

```json
{
  "schema": "chart-caption.v1",
  "headline": "Revenue rose 45.8% from 4.8M to 7.0M across 2020–2024",
  "facts": [
    {
      "text": "Revenue rose 45.8% from 4.8M to 7.0M across 2020–2024",
      "category": "trend",
      "values": {
        "first_value": 4800000.0,
        "last_value": 7000000.0,
        "delta": 2200000.0,
        "pct_change": 45.833333,
        "first_dim_label": "2020",
        "last_dim_label": "2024"
      }
    }
  ],
  "provenance": [
    {
      "category": "trend",
      "measure": "revenue",
      "dimension": "year",
      "computed_values": { ... },
      "fired": true,
      "suppressed_reason": null
    }
  ]
}
```

## Dependencies

- `mqo-result-profiler` (path dep) — `ResultProfile` / `ColumnProfile` types

## License

MIT OR Apache-2.0
