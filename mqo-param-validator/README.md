# mqo-param-validator

Server-side validator that rejects unmapped MQO fields before execution
(lookalike_measure, wrong_hierarchy_level, wrong_date_role, cross_fact_path).

## TL;DR

`mqo-mcp-server` accepts the structured MQO schema but has no pre-execution
validator: a `query_multidimensional` call naming a non-existent measure gets
discovered deep in the pipeline with a backend error rather than a crisp typed
rejection. This library is the missing guardrail:

```
validate(mqo: &BoundMqoInput, catalog: &CatalogSnapshot) -> Vec<ParamRejection>
```

Empty `Vec` = fully grounded MQO, safe to execute. Non-empty = reject before
the engine ever sees it. Pure, deterministic — no LLM, no network, no `unsafe`.

Implements ATSCALE-49213 AC2: "a server-side validator prototype that rejects
calls referencing unmapped fields."

## Acceptance Criteria

| # | Description | Status |
|---|-------------|--------|
| AC1 | Fully-mapped MQO returns empty `Vec` | MUST |
| AC2 | Unmapped measure → `Unmapped` + ranked `strsim` suggestions | MUST |
| AC3 | Level in wrong hierarchy → `WrongHierarchyLevel` + valid levels as suggestions | MUST |
| AC4 | Date base dim with ≥2 roles, no qualifier → `AmbiguousDateRole` + all role candidates | MUST |
| AC5 | Measure+dim from disjoint subject areas → `CrossFactPath`; conformed dim = no false positive | MUST |
| AC6 | Never panics on malformed/partial input; total function | MUST |
| AC7 | `cargo test` green; `cargo clippy -- -D warnings` clean; zero `unsafe` | MUST |

## Core Types

```rust
pub struct CatalogSnapshot {
    pub measures: Vec<CatalogMeasure>,
    pub dimensions: Vec<CatalogDimension>,
    pub hierarchies: Vec<CatalogHierarchy>,
    pub date_roles: Vec<CatalogDateRole>,
}

pub enum FieldClass { Measure, Dimension, HierarchyLevel, DateRole, Filter }

pub enum RejectReason {
    Unmapped,
    WrongHierarchyLevel,
    AmbiguousDateRole,
    CrossFactPath,
}

pub struct ParamRejection {
    pub field: String,
    pub class: FieldClass,
    pub reason: RejectReason,
    pub suggestions: Vec<Suggestion>,
}

pub struct Suggestion { pub name: String, pub similarity: f64, pub note: Option<String> }
```

## Install

Add to `Cargo.toml`:

```toml
[dependencies]
mqo-param-validator = { git = "https://github.com/joeyen-atscale/mqo-param-validator" }
```

## Usage

```rust
use mqo_param_validator::{validate, BoundMqoInput, CatalogSnapshot, RejectReason};

let mqo: BoundMqoInput = serde_json::from_str(mqo_json)?;
let catalog: CatalogSnapshot = serde_json::from_str(catalog_json)?;

let rejections = validate(&mqo, &catalog);
if rejections.is_empty() {
    // safe to execute
} else {
    for r in &rejections {
        eprintln!("Rejected [{}] ({:?}): {:?}", r.field, r.class, r.reason);
        for s in &r.suggestions {
            eprintln!("  suggestion: {} (sim={:.3})", s.name, s.similarity);
        }
    }
}
```

## Checks

1. **Unmapped entity** — every MQO measure/dimension resolves to a catalog
   `unique_name` (post-normalization: lowercase, collapse whitespace, strip
   punctuation). Unresolved → `Unmapped` with `strsim` Jaro-Winkler nearest matches.

2. **Wrong hierarchy level** — a level that exists in some hierarchy but not
   the one implied by the dimension → `WrongHierarchyLevel` with the chosen
   hierarchy's valid levels as suggestions.

3. **Ambiguous date role** — a date concept resolving to ≥2 role-played date
   dimensions with no role qualifier → `AmbiguousDateRole` listing all candidates.

4. **Cross-fact path** (conservative, structural) — a measure+dimension pair
   whose catalog subject areas don't intersect → `CrossFactPath`. Never fires
   on a conformed dimension (one with `subject_areas = []`).

## Dependencies

- `serde` + `serde_json` — catalog/MQO deserialization
- `strsim` — Jaro-Winkler similarity for nearest-match suggestions

No LLM, no network, no `unsafe`.
