# mqoguard-column-group-enrichment

Deterministic enrichment pass that attaches a `column_group` set to every measure and dimension level in an AtScale catalog snapshot, identifying which fact table(s) each entity belongs to.

## TL;DR

The AtScale `describe_model` catalog tags columns with `kind`, `is_calc`, `hierarchy`, and `level` — but nothing that identifies **which fact a measure or dimension level belongs to**. Without that tag, neither an AI agent nor `mqo-param-validator`'s cross-fact check can tell that "Inventory Quantity On Hand" (inventory fact) cannot be sliced by "Promotions" (sales facts only).

This library is the missing foundation: given a catalog snapshot plus `FactBindings`, it attaches a `column_group: BTreeSet<String>` to every measure and dimension level. Conformed dimensions (joinable to multiple facts) receive all matching group identifiers — no false single-fact restriction. Unbound entities receive an explicit empty set and appear in a coverage report.

Pure, deterministic, no-LLM, no-network. A 500-entity model enriches in under 100 ms.

## Acceptance tests

| AC | Description | Test file |
|----|-------------|-----------|
| AC1 | Measure bound to a fact gets exactly that fact's column_group | `tests/acceptance_AC1.rs` |
| AC2 | Dimension level joinable to N facts gets all N column_group identifiers | `tests/acceptance_AC2.rs` |
| AC3 | All non-`column_group` fields preserved byte-identical (additive) | `tests/acceptance_AC3.rs` |
| AC4 | Unbound entity carries `column_group: []` and appears in coverage report, never dropped | `tests/acceptance_AC4.rs` |
| AC5 | TPC-DS benchmark: 100% coverage, inventory measures carry no sales group | `tests/acceptance_AC5.rs` |
| AC6 | Malformed catalog/bindings JSON returns typed error, never panics | `tests/acceptance_AC6.rs` |
| AC7 | `cargo test` passes; `cargo clippy --all-targets -- -D warnings` clean; zero `unsafe` | CI |

All 26 tests pass at release profile.

## Install

```toml
# Cargo.toml
[dependencies]
mqoguard-column-group-enrichment = { git = "https://github.com/joeyen-atscale/mqoguard-column-group-enrichment" }
```

## Quick usage

```rust
use mqoguard_column_group_enrichment::{enrich, CatalogSnapshot, FactBindings};

let catalog: CatalogSnapshot = serde_json::from_str(catalog_json)?;
let bindings: FactBindings = serde_json::from_str(bindings_json)?;
let result = enrich(&catalog, &bindings);

println!("Bound: {}, Unbound: {}", result.coverage.bound, result.coverage.unbound);
for col in &result.columns {
    println!("{}: {:?}", col.unique_name, col.column_group);
}
```

## API

- `enrich(catalog: &CatalogSnapshot, bindings: &FactBindings) -> EnrichedCatalog`
- `EnrichedCatalog.columns` — all input columns with `column_group: BTreeSet<String>` added
- `EnrichedCatalog.coverage` — `{ bound: usize, unbound: usize, total: usize, schema: "enriched-catalog.v1" }`

## Dependencies

`serde`, `serde_json`, `thiserror` — no LLM, no network, zero `unsafe`.

## License

MIT OR Apache-2.0
