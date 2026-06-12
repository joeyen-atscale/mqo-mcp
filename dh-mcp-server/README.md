# dh-mcp-server

The MCP server that puts it together. `query_multidimensional` runs the existing MQO
bind→route→compile→execute pipeline but, instead of returning rows, **stores the result
in `dh-store` and returns `{ summary, handle, capabilities }`**. A `dataset_*` tool
family (`dataset_peek`, `dataset_aggregate`, `dataset_filter`, `dataset_sort`,
`dataset_top_n`, `dataset_pivot`, `dataset_compare`, `dataset_drill`, `dataset_describe`,
`dataset_export`) lets the LLM work the data in place. Read-only and deterministic by
construction: the model orchestrates handles, the server owns the numbers.

Part of the dataset-handle MCP fleet (vision: dataset-handle-mcp).

## Install

```
cargo install --path .
```

## License

MIT OR Apache-2.0
