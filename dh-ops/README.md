# dh-ops

The compute kernel that makes "the LLM never recomputes" real. `dh-ops` implements
nine operations a model can request on a dataset handle — `aggregate`, `filter`, `sort`,
`top_n`, `pivot`, `compare`, `drill`, `describe` — each as a pure, deterministic
function `(Store, DatasetHandle, params) -> Result<OpResult, OpError>` that the server
runs server-side and returns a new handle. The model picks the operation and parameters;
the arithmetic is ours.

Moving post-query operations server-side turns a stochastic LLM-as-calculator problem
into a deterministic, unit-tested guarantee: same input + same params → byte-identical output.

Part of the `dh-*` MCP fleet. Depends on `dh-spec`, `dh-store`, `dh-summary`.

## Operations

| Function | Description |
|---|---|
| `aggregate` | Group-by + agg (sum/mean/min/max/count/count_distinct) |
| `filter` | Compound AND/OR predicate (eq/ne/lt/le/gt/ge/in/contains/null-checks) |
| `sort` | Multi-key stable sort (asc/desc) |
| `top_n` | Top/bottom N by a measure (deterministic tie-break: smaller row index wins) |
| `pivot` | Rows × cols × measure crosstab |
| `compare` | Two handles → delta + pct-change (multi-parent lineage) |
| `drill` | Expand a grouped row to detail rows by walking lineage |
| `describe` | Per-column stats without mutating rows |

## Acceptance criteria (all MUST, all green)

1. `aggregate` sum/mean/min/max/count/count_distinct each verified against hand-computed golden.
2. `filter` with compound AND/OR predicate returns exactly the expected row subset; numeric and string predicates covered.
3. `sort` is stable and correct for multi-key asc/desc; `top_n` returns the right N with documented deterministic tie-break.
4. `pivot` produces the correct crosstab for a 2-dimension × 1-measure fixture.
5. `compare` yields correct delta + pct-change and records a 2-parent lineage.
6. `drill` expands a grouped row to its constituent detail rows by walking lineage.
7. Every op returns a NEW handle (never mutates input) and an `OpResult` whose summary `sample` ≤ `sample_cap`.
8. Determinism: running any op twice on the same input yields byte-identical stored output.
9. `cargo test --release` passes; `cargo clippy --release -- -D warnings` clean.

## Usage

```toml
# Cargo.toml
[dependencies]
dh-ops = { git = "https://github.com/joeyen-atscale/dh-ops" }
```

```rust
use dh_ops::{aggregate, filter, OpError};
use dh_spec::DatasetHandle;
use dh_store::Store;
use serde_json::json;

let mut store = Store::new(0);
let handle: DatasetHandle = /* from dh-store */;

let result = aggregate(&mut store, &handle, &json!({
    "group_by": ["region"],
    "agg": "sum",
    "measure": "revenue"
}))?;

// result.handle is a new DatasetHandle
// result.summary contains statistics (no raw rows)
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
