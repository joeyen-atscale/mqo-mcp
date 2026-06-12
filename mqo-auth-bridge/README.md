# mqo-auth-bridge

**TL;DR:** The MQO MCP server runs a full bind → route → compile pipeline and then throws the compiled query away: `src/engine.rs` is a fixture engine that synthesizes deterministic rows and never touches a cluster. This crate builds the missing half: a Rust library that (a) fetches a Keycloak OIDC client-credentials access token and (b) sends the compiled query text to a live AtScale endpoint — SQL over PGWire (`:15432`), MDX **and DAX** over XMLA (`/v1/xmla`) — returning the same bounded `EngineResult { rows }` shape the fixture engine returns today, so the server swaps one for the other behind a trait.

## Features

- `Engine` trait — the abstraction the MQO MCP server programs against.
- `FixtureEngine` — deterministic, cluster-free row synthesis (matches `mqo-mcp-server/src/engine.rs` exactly).
- `LiveExecutor` — OIDC client-credentials token flow + live query dispatch (PGWire for SQL, XMLA `/v1/xmla` for MDX and DAX).
- `OidcConfig` — secret stored only as an env-var name; never as a literal.
- Bounded by construction — all results clamped to `HARD_ROW_CAP = 1000`.

## Install

Add to `Cargo.toml`:

```toml
[dependencies]
mqo-auth-bridge = { path = "../mqo-auth-bridge" }
```

### Quick usage

```rust
use mqo_auth_bridge::{
    Backend, Engine, FixtureEngine, LiveExecutor,
    EndpointConfig, OidcConfig,
};
use serde_json::json;

// Cluster-free fixture engine
let eng = FixtureEngine::with_bound(json!({
    "dimensions": [{"unique_name": "time.[Year]", "hierarchy": "h"}],
    "measures":   [{"unique_name": "sales.revenue"}]
}));
let result = eng.execute("SELECT ...", Backend::Dax, Some(5)).unwrap();
println!("{} rows", result.rows.len()); // → 5

// Live executor (requires ATSCALE_API_CLIENT_SECRET in env)
let exec = LiveExecutor::new(EndpointConfig {
    pgwire_host: "localhost".to_string(),
    pgwire_port: 15432,
    xmla_url:    "https://mcp-aws.atscaleinternal.com/v1/xmla".to_string(),
    oidc: OidcConfig {
        token_url:              "http://localhost:8080/realms/community-identity/protocol/openid-connect/token".to_string(),
        client_id:              "atscale-api".to_string(),
        client_secret_env_var:  "ATSCALE_API_CLIENT_SECRET".to_string(),
        realm:                  "community-identity".to_string(),
    },
});
let result = exec.execute("SELECT ...", Backend::Dax, Some(10))?;
```

## AtScale endpoint notes (confirmed 2026-06-10)

### XMLA endpoint

`/v1/xmla` on port 443 accepts **both MDX and DAX** as the SOAP `<Statement>` body:

- **MDX** — `SELECT {[Measures].[M]} ON COLUMNS FROM [cube]`
- **DAX** — `EVALUATE ROW("M", [M])` — the `<Cube>` property in `<PropertyList>` is **required**; without it the engine returns "DAX query requires a cube property"

Do **not** use:
- `/xmla` or `/dax` on port 443 — routed to the Next.js Modeler app (session cookie required), not the engine
- Port 11111 — firewalled externally

### PGWire endpoint

Port 15432 is **SQL-only**. DAX `EVALUATE` statements return a syntax error. Route `Backend::Dax` through `/v1/xmla`, not PGWire.

### Auth

Both `/mcp` and `/v1/xmla` accept the same `client_credentials` Bearer token. For AtScale Cloud:

```
token_url: https://mcp-aws.atscaleinternal.com/auth/realms/atscale/protocol/openid-connect/token
client_id: atscale-mcp
```

Set `client_secret_env_var` to the name of the env var holding `atscale` (or your deployment's secret).

## Environment variables

| Variable | Purpose |
|----------|---------|
| `<your_env_var>` | Client secret for OIDC — set `OidcConfig.client_secret_env_var` to the var name |
| `ATSCALE_PGWIRE_HOST` | When set, live-cluster integration tests are un-skip-gated |

## License

MIT OR Apache-2.0
