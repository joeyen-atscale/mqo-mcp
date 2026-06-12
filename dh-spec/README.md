# dh-spec

Define the wire contract for handle-based results so the LLM receives a
**summary + handle + capabilities**, never a raw dataset. This crate is the shared
vocabulary every other fleet member compiles against: `DatasetHandle`,
`DatasetSummary`, `ColumnSchema`, `Capability`, `OpRequest`/`OpResult`, and a
`Lineage` record — plus a JSON Schema so non-Rust MCP clients can validate.

Part of the dataset-handle MCP fleet (vision: dataset-handle-mcp).

## Install

```
# library crate — add as a path/git dependency
```

## License

MIT OR Apache-2.0
