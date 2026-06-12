# dh-summary

The thing that replaces "dump all the rows." Given a dataset, `dh-summary` produces a
`DatasetSummary` — shape, per-column stats, a small head/tail sample, notable values —
plus the advertised `Capability` list — sized to be safe for the context window. This
is what every query and every operation returns, so its bound is the whole point: it
must be informative enough to orchestrate the next step but never large enough to be
"the dataset."

Part of the dataset-handle MCP fleet (vision: dataset-handle-mcp).

## Install

```
# library crate — add as a path/git dependency
```

## License

MIT OR Apache-2.0
