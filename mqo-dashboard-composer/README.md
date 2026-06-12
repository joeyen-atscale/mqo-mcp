# mqo-dashboard-composer

Compose N `bi-asset.v1` bundles into a `dashboard.v1` layout manifest with both a
`panels[]` grid (for clients that lay out themselves) and a Vega-Lite v5 `vconcat`/`hconcat`/`concat`
spec (for clients with a VL engine). Final component (5/5) of the `bi-toolkit-for-mcp` vision.

## Output: `dashboard.v1`

```json
{
  "dashboard": "dashboard.v1",
  "title": "Sales Overview",
  "layout": "grid",
  "columns": 2,
  "panels": [
    { "title": "Revenue by Year", "caption": "Sum of Revenue across Year.", "vega_spec": {...}, "row": 0, "col": 0 },
    { "title": "Margin by Region", "caption": "Sum of Margin across Region.", "vega_spec": {...}, "row": 0, "col": 1 }
  ],
  "vega_concat_spec": { "$schema": "...", "title": "Sales Overview", "concat": [...], "columns": 2 }
}
```

## Usage

```
mqo-dashboard-composer --assets bundles.json --title "Sales Overview"
mqo-dashboard-composer --asset rev.json --asset margin.json --title "Overview" --layout vertical
mqo-dashboard-composer --assets bundles.json --title "Overview" --layout grid --columns 3 --format human
```

## Install

```bash
cargo install --path .
```

## Acceptance criteria

- AC1: two bundles → dashboard with two panels, titles preserved
- AC2: grid layout with columns=2 → correct row-major (row, col) placement
- AC3: vertical layout → `vconcat` array in input order
- AC4: panel caption = description + caveats folded in
- AC5: zero panels → structured error, nonzero exit
- AC6: one panel → valid single-panel dashboard
- AC7: panel ordering and grid placement are deterministic

25 tests, all 7 ACs green. Clippy clean.
