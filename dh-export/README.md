# dh-export

The one deliberate, audited materialization boundary in the `dh-*` fleet.

There is exactly one place a full dataset is allowed to leave the server: an explicit
`export`. `dh-export` turns a handle into CSV / Parquet / bounded-JSON for a file or
a caller-requested payload, and records an audit entry of exactly what crossed the
boundary. Making export the *single* sanctioned exit keeps the default path
(summary + handle) honest while still letting a user say "give me the actual data."

## What this builds

A Rust library crate exposing:

```rust
pub fn export(
    store: &Store,
    handle: &DatasetHandle,
    fmt: ExportFmt,
    dest: ExportDest,
    opts: ExportOptions,
) -> Result<ExportReceipt, ExportError>
```

- `ExportFmt` — `Csv` | `Json { max_rows }` | `Parquet` (behind the `parquet` feature)
- `ExportDest` — `File(path)` (atomic tempfile + rename) | `Inline { max_bytes }`
- `ExportReceipt` — `{ handle, fmt, dest, row_count, bytes, sha256, ts }` — the audit record of what crossed
- `ExportError` — typed error enum covering every failure mode (no panics on safe callers)

## Acceptance criteria

1. CSV export of a fixture handle produces correct, well-formed CSV (header + rows match the dataset); round-trip parse equals the source.
2. JSON export honors `max_rows`: a dataset larger than `max_rows` is refused with a typed error unless an explicit override is set; within bound it emits exactly the rows.
3. `ExportReceipt` reports correct `row_count`, `bytes`, and a stable `sha256` of the payload.
4. File export writes atomically (no partial file on simulated mid-write failure) and refuses to overwrite an existing file without the overwrite flag.
5. Parquet export (behind the `parquet` feature) produces a file readable back to the same rows; the test is feature-gated and skipped cleanly when the feature is off.
6. Exporting an expired/unknown handle returns a typed error, never a panic.
7. `cargo test --release` passes (default features); `cargo clippy --release -- -D warnings` clean.

## Dependencies

- `dh-spec` — shared handle + schema types
- `dh-store` — in-memory columnar dataset store
- `serde`, `serde_json` — serialization
- `csv` — CSV writer
- `sha2`, `hex` — SHA-256 receipt hash
- `tempfile` — atomic file writes
- `arrow`, `parquet` (optional, `--features parquet`) — Parquet export

## Usage

```toml
[dependencies]
dh-export = { git = "https://github.com/joeyen-atscale/dh-export" }
# or with Parquet support:
dh-export = { git = "https://github.com/joeyen-atscale/dh-export", features = ["parquet"] }
```

## Design rationale

A handle-only system that can never produce data is useless for the legitimate
"download the results" case. But if any tool can spill rows, the LLM-as-calculator
guarantee leaks. The design answer is to concentrate all materialization in one
audited operation with an explicit destination, so "data crossed the boundary" is
always a deliberate, logged event — not a side effect of a summary or an op.

Part of the dataset-handle MCP fleet. Vision: `visions/dataset-handle-mcp.md`.

## License

Licensed under either of:

- [MIT License](LICENSE-MIT)
- [Apache License, Version 2.0](LICENSE-APACHE)

at your option.
