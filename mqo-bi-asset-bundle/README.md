# mqo-bi-asset-bundle

One-shot orchestrator: MCP `query_multidimensional` response + catalog → titled, captioned BI asset.

Chains `mqo-result-profiler` → `mqo-chart-recommender` → `mqo-vega-emitter` and adds synthesis not possible from any single stage: a human title/description built from catalog labels, and semantic caveats (semi-additive over time, is_calc percentage summed, high-cardinality clutter).

## Output: `bi-asset.v1`

```json
{
  "asset": "bi-asset.v1",
  "title": "Revenue by Year",
  "description": "Sum of Revenue across Year.",
  "vega_spec": { "$schema": "...", "mark": "line", "encoding": {...} },
  "profile_summary": { "row_count": 5, "measures": ["Revenue"], "dimensions": ["Year"] },
  "caveats": []
}
```

## Usage

```
mqo-bi-asset-bundle --response response.json --catalog catalog.json
mqo-bi-asset-bundle --response response.json --catalog catalog.json --format human
```

## Install

```bash
cargo install --path .
```

## Acceptance criteria

- AC1: revenue-by-year → title "Revenue by Year", line spec, profile summary
- AC2: description is a single templated sentence matching the mark
- AC3: semi_additive measure over temporal axis → aggregation caveat
- AC4: nominal axis with cardinality > 25 → clutter caveat
- AC5: clean case → caveats: []
- AC6: embedded vega_spec is valid VL5 (has $schema, mark, encoding)
- AC7: malformed response → structured error, nonzero exit

33 tests (7 named AC + per-AC unit tests), all green. Clippy clean.
