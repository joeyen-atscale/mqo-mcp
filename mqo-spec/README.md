# mqo-spec

**Multidimensional Query Object — the typed contract for the MQO fleet.**

Every piece of the MQO fleet shares one thing: **the object**. This crate defines
the Multidimensional Query Object — a typed, `serde`-serializable Rust schema plus
an emitted JSON Schema — that an LLM constructs *instead of SQL*: a selection of
measures, dimensions/levels, filters, calc-group members, and time-intelligence
operations, with ordering, limit, and non-empty flags.

It is the contract the binder, compilers, router, server, and benchmark all consume.
**No query logic here — just the shape and its validation.**

## Why this exists

The `query-semantic-layer` skill imposes a 13-rule gate (R1–R13) precisely because
free-text SQL has an unconstrained output space. An MQO replaces that open string
with a closed, enumerable structure — but only if there is a single canonical schema
everything agrees on. This crate is that schema. Without it, binder/compilers/server
each reinvent the type and drift.

## Install

```toml
[dependencies]
mqo-spec = { git = "https://github.com/joeyen-atscale/mqo-spec" }
```

## Quick start

```rust
use mqo_spec::{Mqo, MeasureRef, Filter, TimeIntel, Grain, validate};

let mqo = Mqo {
    model: "sales".to_string(),
    measures: vec![MeasureRef { unique_name: "sales.revenue".to_string() }],
    dimensions: vec![],
    filters: vec![],
    time_intelligence: vec![TimeIntel::ToDate { grain: Grain::Year }],
    order: None,
    limit: Some(100),
    non_empty: true,
};

assert!(validate(&mqo).is_ok());

// Emit JSON Schema for non-Rust producers (LLM skill, other languages)
let schema_json = mqo_spec::emit_json_schema();
```

## Acceptance criteria

| # | Level | Criterion |
|---|-------|-----------|
| AC1 | MUST | All MQO types round-trip through JSON losslessly for every fixture |
| AC2 | MUST | `emit_json_schema()` returns a valid JSON Schema document for `Mqo` |
| AC3 | MUST | `validate()` rejects: empty `measures`, `limit` of 0, `Range` filter with `lo > hi` — each with a distinct `MqoError` |
| AC4 | MUST | ≥6 golden fixtures (one per `TimeIntel` variant + calc-group + member filter) parse and validate |
| AC5 | MUST | `cargo test --release` passes; `cargo clippy -- -D warnings` is clean |
| AC6 | SHOULD | `BoundMqo` carries resolved `unique_name`s + per-ref metadata flags (`is_calc`, `semi_additive`, `required_dimension`) |

All criteria are green on `HEAD` (24 tests, 0 clippy warnings).

## Types

```
Mqo { model, measures: Vec<MeasureRef>, dimensions: Vec<LevelSelection>,
      filters: Vec<Filter>, time_intelligence: Vec<TimeIntel>,
      order: Option<Vec<OrderKey>>, limit: Option<u64>, non_empty: bool }

Filter:     Member { hierarchy, members }
          | Range { level, lo, hi }
          | CalcGroupMember { calc_group, member }

TimeIntel:  YoY | PriorPeriod | ToDate { grain: Grain }
          | RunningTotal | Share { of_level } | Rank { by, top_n }

BoundMqo — binder output: resolved unique_names + is_calc, semi_additive,
           required_dimension per measure
```

## Non-goals

- No query execution, parsing, or compilation
- No DAX/MDX/SQL generation
- No network or async runtime dependency
- No semantic/model validation (measure name existence etc.)
- No binder implementation (BoundMqo is a type stub)

## Binary helper

```sh
cargo run --bin mqo-spec -- schema          # print JSON Schema for Mqo
cargo run --bin mqo-spec -- validate foo.json  # parse + structural-validate
```

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at your option.
