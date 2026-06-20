# mqo-mcp-server

The capstone of the MQO fleet: an **MCP server** whose headline tool,
`query_multidimensional`, accepts a **Multidimensional Query Object** — never raw
SQL — and runs the full pipeline:

```text
MQO ─▶ mqo-bind ─▶ mqo-route ─▶ mqo-dax | mqo-mdx | sql-projection ─▶ fixture engine ─▶ bounded rows
```

Read-only by construction: the only query input is a selection-only object, so
the "can this tool write?" HITL concern disappears. The three catalog tools
(`list_models`, `describe_model`, `search_columns`) are advertised with
`readOnlyHint: true`.

## Architecture

The MQO fleet is a JSON pipeline of CLI subprocesses, not a library graph. This
server orchestrates the published fleet binaries as subprocesses, passing JSON on
disk per each tool's documented CLI contract:

- `mqo-bind`  — `--mqo <f> --catalog <f>` → `BoundMqo` JSON (exit 0),
  `{"ambiguous":[…]}` (3), `{"not_found":[…]}` (4)
- `mqo-route` — `--bound <f> --stats <f> [--row-threshold N]` → routing decision
- `mqo-dax`   — `--bound <f>` → DAX `EVALUATE` text
- `mqo-mdx`   — `--bound <f>` → MDX `SELECT` text

`mqo-spec` is a library path-dependency (the `Mqo` / `BoundMqo` types). The
fixture engine (`src/engine.rs`) deterministically synthesizes bounded result
rows so the acceptance tests run with no live cluster. With `--endpoint` and the
OIDC flags, the server swaps in a live executor via `mqo-auth-bridge` (SQL over
PGWire, MDX and DAX over XMLA `/v1/xmla`).

## Transport

Minimal MCP JSON-RPC 2.0 over stdio (`initialize`, `tools/list`, `tools/call`,
`ping`). Newline-delimited JSON in, newline-delimited JSON out.

## Usage

```
mqo-mcp-server --catalog <snapshot.json> [--stats <stats.json>] \
               [--release-dir <dir>] [--row-threshold <N>]
```

The server resolves the fleet binaries from `--release-dir`, then `~/.local/bin`,
then `PATH`. Fixtures live in `fixtures/`.

## Acceptance

`cargo test --release` (13 tests, one per AC: `ac1`…`ac6`) and
`cargo clippy --release -- -D warnings` are both green. The subprocess-dependent
ACs are skip-gated (with a printed note) when the fleet binaries are absent.

## Recent

- **v0.2.0** — live execution via `mqo-auth-bridge`. Without `--endpoint` the
  server behaves exactly as before (fixture engine, cluster-free). With
  `--endpoint` it connects to a live `AtScale` endpoint.

  Community-edition invocation:

  ```
  mqo-mcp-server \
    --catalog snapshot.json \
    --endpoint localhost:15432 \
    --xmla-url http://localhost:11111/xmla \
    --oidc-token-url http://localhost:8080/realms/<realm>/protocol/openid-connect/token \
    --oidc-client-id <id> \
    --oidc-realm <realm> \
    --oidc-client-secret-env ATSCALE_CLIENT_SECRET
  ```

  AtScale Cloud invocation (confirmed 2026-06-10 — `/v1/xmla` accepts both MDX
  and DAX; port 11111 is firewalled externally):

  ```
  mqo-mcp-server \
    --catalog snapshot.json \
    --endpoint mcp-aws.atscaleinternal.com:15432 \
    --xmla-url https://mcp-aws.atscaleinternal.com/v1/xmla \
    --oidc-token-url https://mcp-aws.atscaleinternal.com/auth/realms/atscale/protocol/openid-connect/token \
    --oidc-client-id atscale-mcp \
    --oidc-realm atscale \
    --oidc-client-secret-env ATSCALE_CLIENT_SECRET
  ```

  **DAX routing note:** `Backend::Dax` queries are sent to `--xmla-url`, not
  PGWire. PGWire (`:15432`) is SQL-only; DAX `EVALUATE` statements are rejected
  at the wire level. The `/v1/xmla` endpoint requires `<Cube>` in the XMLA
  `<PropertyList>` when the statement language is DAX.

## License

MIT OR Apache-2.0
