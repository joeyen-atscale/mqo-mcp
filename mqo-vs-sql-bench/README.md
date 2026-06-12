# mqo-vs-sql-bench

The vision's hypothesis — *multidimensional datastores beat tabular for analytics,
and especially as an AI grounding layer* — must be a number, not a slogan. This CLI
runs a golden NL-question set through two arms — (A) the text-to-SQL `run_query`
path and (B) the MQO `query_multidimensional` path — and emits a head-to-head report
on accuracy, invalid-entity (hallucination) rate, retries, latency, and tokens.

Final crate of the MQO fleet (deps: mqo-spec, mqo-mcp-server; graders shelled out as a configurable command). Fixture test runs with no live cluster/model/grader.

## Install

```
cargo install --path .
```

## License

MIT OR Apache-2.0
