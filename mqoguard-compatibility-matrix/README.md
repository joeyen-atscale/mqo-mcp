# mqoguard-compatibility-matrix

Compute the measure×hierarchy compatibility matrix from an `EnrichedCatalog` and emit it as a `compatibility-matrix.v1` JSON fragment suitable for embedding in `describe_model` responses.

## TL;DR

Even once the catalog carries column-groups (Component 1), an agent still has to cross-reference them by hand to decide whether a measure can be sliced by a hierarchy. Today that reasoning is offloaded to a hand-maintained incompatibility table pasted into the mcp-tuner subagent prompt and the `construct-mqo` skill — a table that drifts from the model and doesn't scale past TPC-DS.

This library computes the compatibility relation directly from column-groups and emits it as first-class `describe_model` output: per measure, the set of compatible hierarchies; per hierarchy, the set of compatible measures. The agent reads compatibility from the tool, not from a memorized prose table.

## Compatibility rule

A (measure, hierarchy) pair is **compatible** iff the measure's `column_group` set intersects the union of `column_group` sets of that hierarchy's levels. Conformed dimensions (multi-fact levels) therefore stay broadly compatible — no false exclusions.

## Key properties

- **Symmetric**: measure M lists hierarchy H iff H lists measure M.
- **Deterministic**: pure function, no I/O, no LLM, no network calls.
- **Fail-safe**: when no `column_group` data is present, returns an empty matrix with a diagnostic note rather than an all-compatible matrix.
- **Payload-bounded**: when the forward map would exceed the configured character budget, the inverse index is dropped (and is reconstructable from the forward map).

## Install

Add to `Cargo.toml`:

```toml
[dependencies]
mqoguard-compatibility-matrix = { git = "https://github.com/joeyen-atscale/mqoguard-compatibility-matrix" }
```

## Usage

```rust
use mqoguard_compatibility_matrix::{
    EnrichedCatalog, EnrichedColumn, MatrixConfig, build_matrix, to_describe_model_fragment,
};
use std::collections::BTreeSet;

let catalog = EnrichedCatalog {
    model: "postgres.tpcds.tpcds_benchmark_model".into(),
    columns: vec![
        EnrichedColumn {
            unique_name: "sales_amount".into(),
            label: "Sales Amount".into(),
            kind: "measure".into(),
            is_calc: false,
            hierarchy: None,
            level: None,
            column_group: BTreeSet::from(["sales".into()]),
        },
        EnrichedColumn {
            unique_name: "date_day".into(),
            label: "Date Day".into(),
            kind: "dimension".into(),
            is_calc: false,
            hierarchy: Some("Date".into()),
            level: Some("Day".into()),
            column_group: BTreeSet::from(["sales".into()]),
        },
    ],
};

let matrix = build_matrix(&catalog, &MatrixConfig::default());

// Embed in describe_model response
let fragment = to_describe_model_fragment(&matrix)?;
// {"compatibility-matrix.v1": {"schema": "compatibility-matrix.v1", "measures": {...}, ...}}
```

## Acceptance criteria

| # | Requirement | Test |
|---|-------------|------|
| AC1 | Measure with column-group `{sales}` is excluded from a hierarchy whose levels are all in `{inventory}` | `tests/acceptance_ac1.rs` |
| AC2 | A conformed dimension hierarchy spanning `{sales, inventory}` is compatible with both a sales measure and an inventory measure | `tests/acceptance_ac2.rs` |
| AC3 | The relation is symmetric for all (measure, hierarchy) pairs in the output | `tests/acceptance_ac3.rs` (proptest) |
| AC4 | On the TPC-DS enriched catalog: "Inventory Quantity On Hand" is incompatible with Promotions and compatible with Inventory Calendar | `tests/acceptance_ac4.rs` |
| AC5 | When the payload budget is exceeded, the inverse index is dropped and is reconstructable from the forward map | `tests/acceptance_ac5.rs` |
| AC6 | A catalog with no `column_group` fields returns an empty matrix with a diagnostic note, not an all-compatible matrix | `tests/acceptance_ac6.rs` |
| AC7 | `cargo test` passes; `cargo clippy --all-targets -- -D warnings` clean; zero `unsafe` | CI |

Run all tests:

```sh
cargo test --release
```

## JSON output shape

```json
{
  "compatibility-matrix.v1": {
    "schema": "compatibility-matrix.v1",
    "measures": {
      "sales_amount": {
        "compatible_hierarchies": ["Date", "Product", "Store"]
      }
    },
    "hierarchies": {
      "Date": {
        "compatible_measures": ["sales_amount", "net_profit"]
      }
    }
  }
}
```

When the inverse index is dropped (payload budget exceeded), `hierarchies` is empty and a `note` field explains how to reconstruct it from `measures`.

## License

MIT OR Apache-2.0
