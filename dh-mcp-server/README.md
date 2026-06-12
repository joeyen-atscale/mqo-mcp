# dh-mcp-server

> ## ⚠️ DEPRECATED (2026-06-11)
>
> **`dh-mcp-server` is deprecated. Use [`mqo-mcp-server`](../mqo-mcp-server) instead.**
>
> Per PRD-mqo-mcp-handle-merge, the dataset-handle capability has been merged
> into `mqo-mcp-server`, which is now the one canonical server: it has live
> execution (`--endpoint`/`--xmla-url`/`--oidc-*`), catalog grounding
> (`list_models`/`describe_model`/`search_columns`), cursor pagination
> (`next_page`), federation (`health_status`/`list_clusters`/`diff_clusters`),
> charts, **and** the full size-gated handle-op family (`dataset_aggregate`,
> `dataset_filter`, `dataset_sort`, `dataset_top_n`, `dataset_pivot`,
> `dataset_compare`, `dataset_drill`, `dataset_describe`, `dataset_slice`,
> `dataset_period_over_period`, `dataset_chart`) backed by `dh-store` + `dh-ops`.
> `mqo-mcp-server`'s `query_multidimensional` returns `{summary, handle,
> capabilities, row_count}` and inlines `rows` only when
> `row_count <= --inline-threshold` (default 25).
>
> `dh-mcp-server` is fixture-only (no live cluster) and is retained, not deleted,
> for reference. No new work should target it.

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
