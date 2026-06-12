# dh-store

The server-side home for datasets the LLM never sees. `dh-store` holds query results
as in-memory columnar tables keyed by opaque `DatasetHandle`, with TTL + LRU eviction
+ a total-size cap, and **immutable derive-new-handle semantics**: an operation never
mutates an existing dataset, it inserts a new one and records lineage. This is what
makes "the LLM cannot change a value" structurally true — the values live here, not
in the context window.

Part of the dataset-handle MCP fleet (vision: dataset-handle-mcp).

## Acceptance criteria

All criteria verified by the autobuilder pipeline (24 tests, 67% mutation kill rate):

- **AC1 (MUST):** `put` then `get` returns the same dataset; the handle string contains none of the column names or row values.
- **AC2 (MUST):** `derive(parent, …)` returns a NEW handle, leaves the parent retrievable unchanged, and `lineage(child)` includes the parent (immutability + provenance).
- **AC3 (MUST):** A dataset past its TTL returns `Expired` (distinct from `NotFound`) after `evict_expired()`.
- **AC4 (MUST):** Inserting datasets past `max_total_bytes` evicts LRU entries until under cap; the most-recently-used dataset survives.
- **AC5 (MUST):** There is no public mutation API — `derive` is the only path to a changed dataset.
- **AC6 (MUST):** `stats()` reports live dataset count + total bytes accurately after a mixed put/derive/evict sequence.
- **AC7 (MUST):** `cargo test --release` passes; `cargo clippy --release -- -D warnings` clean.

## Usage

Add as a path or git dependency:

```toml
[dependencies]
dh-store = { git = "https://github.com/joeyen-atscale/dh-store" }
```

```rust
use dh_store::{Dataset, Store};

let store = Store::new(64 * 1024 * 1024); // 64 MiB cap
let handle = store.put(dataset, 3600);     // TTL = 1 hour

let ds = store.get(&handle)?;              // Ok(Dataset) or LookupError

let child = store.derive(&handle, op, params, new_ds, 3600)?;
let lineage = store.lineage(&child);

store.evict_expired();                     // sweep expired entries
let stats = store.stats();                 // live_count + total_bytes
```

## Design

- **Opaque handles:** IDs are UUID v4 strings prefixed `hdl_` — content-blind and unguessable.
- **TTL + tombstone:** `evict_expired()` sweeps expired entries and records tombstones so subsequent `get` returns `LookupError::Expired` (not `NotFound`).
- **LRU + size cap:** Two-pass eviction: first skips live lineage parents; second forces eviction if cap still cannot be met.
- **Immutability:** No `mutate`/`update` API exists. `derive` is the only way to produce a changed dataset; it always allocates a new handle.
- **Thread-safe:** All operations take `&self` and acquire an internal `Mutex`; share via `Arc<Store>`.

## License

MIT OR Apache-2.0
