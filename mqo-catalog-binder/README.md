# mqo-catalog-binder

`mqo-bind` — resolve an MQO against a catalog snapshot (R1–R3 enforced in code).

Before an MQO can be compiled it must be **grounded**: every measure, level,
hierarchy, filter member, and calc-group member must resolve to an exact
`unique_name` that actually exists in the model. This CLI takes an MQO plus a
catalog snapshot (the JSON that `list_models` / `search_columns` / `describe_model`
return) and emits a `BoundMqo` — or a structured candidate-set / not-found report.

## Why this exists

R1 (identify exact `unique_name`s), R2 (present alternatives on ambiguity), and R3
(confirm with rejected alternatives) are the most-violated rules in the text-to-SQL
path because nothing structurally stops the model from inventing a name. The binder
makes a fabricated reference a hard error, and an ambiguous label a candidate set —
turning three prompt rules into one deterministic resolution step.

## Usage

```
mqo-bind --mqo <mqo.json> --catalog <snapshot.json>
```

- exit 0 → `BoundMqo` JSON on stdout (each ref → `unique_name` + `is_calc`,
  `semi_additive`, `trigger_hierarchies`, `required_dimension`, calc-group member MDX)
- exit 2 → bad input (file not found / invalid JSON)
- exit 3 → `{ "ambiguous": [{ ref, candidates: [...] }] }`
- exit 4 → `{ "not_found": [ref, …] }`

## Acceptance criteria

| ID | Level | Description |
|----|-------|-------------|
| AC1 | MUST | Valid MQO binds every ref to `unique_name`, exits 0. Case-insensitive against both `unique_name` and label. |
| AC2 | MUST | Fabricated name → `not_found` report, exit 4. Never guesses a close match. |
| AC3 | MUST | Ambiguous label (same label in ≥2 columns) → `ambiguous` candidate set, exit 3. |
| AC4 | MUST | `CalcGroupMember` filters resolved from `describe_model` Calculation Groups section only; MDX carried into `BoundMqo`. Missing member → `not_found`. |
| AC5 | MUST | Semi-additive measures flagged with `trigger_hierarchies`; non-semi-additive measures have empty `trigger_hierarchies`. |
| AC6 | MUST | `cargo test --release` passes; `cargo clippy --workspace -- -D warnings` clean. |

## Build quality

- 35 tests (24 unit/integration + 5 CLI binary integration + 6 mutant-killer + reviewer counter-attack)
- Mutation kill rate: 90.3% (3 semantically-equivalent survivors)
- No `unsafe` code (`#![forbid(unsafe_code)]`)
- Autobuilder pipeline: 25/25 risk-gate receipts pass

## Install

```
cargo install --path .
```

## Dependencies

- `mqo-spec` (git dep) — MQO/BoundMqo types
- `serde` + `serde_json`
- `clap`

## License

MIT OR Apache-2.0
