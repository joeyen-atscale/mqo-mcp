# dh-vs-json-bench

Turn "handles avoid the LLM-as-calculator problem" into a number. This CLI runs a golden
task set through two arms — (A) **raw-JSON**: the model is handed the full result rows
and asked to compute the answer itself; (B) **handle**: the model gets a summary+handle
and must use `dataset_*` tools — and reports the **value-error / tampering rate**,
plus retries, latency, and tokens, per arm. This is the evidence the vision exists to
produce, and the metric `mqo-vs-sql-bench` does not capture.

Part of the dataset-handle MCP fleet (vision: dataset-handle-mcp).

## Install

```
cargo install --path .
```

## License

MIT OR Apache-2.0
