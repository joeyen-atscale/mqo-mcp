# mcp-cluster-health-monitor

Health canary for registered AtScale clusters. Reads a cluster registry (TOML or JSON),
probes each cluster concurrently via TCP connect, and emits a structured JSON health report.

Part of the AtScale federation gateway toolchain.

## Usage

```
mcp-cluster-health-monitor \
  --registry registry.toml \
  [--timeout-ms 5000] \
  [--format json|human] \
  [--cluster <name>]
```

## Output

```json
{
  "timestamp_ms": 1749432000000,
  "overall": "healthy",
  "clusters": [
    {
      "name": "prod",
      "status": "healthy",
      "latency_ms": 12
    },
    {
      "name": "staging",
      "status": "unhealthy",
      "error": "Connection refused (os error 111)"
    }
  ]
}
```

### Cluster status values

| Status | Meaning |
|---|---|
| `healthy` | TCP connect succeeded within timeout |
| `unreachable` | TCP connect timed out |
| `unhealthy` | TCP connect actively refused |

### Overall status values

| Overall | Condition |
|---|---|
| `healthy` | All clusters healthy |
| `degraded` | Some optional clusters down; all required healthy |
| `critical` | At least one required cluster is down |

### Exit codes

| Code | Meaning |
|---|---|
| 0 | overall healthy |
| 1 | degraded or critical |
| 2 | registry parse error |

## Registry format

```toml
[[clusters]]
name = "prod"
endpoint = "mcp-aws.atscaleinternal.com:15432"
required = true
priority = 1
supported_backends = ["sql"]

[clusters.auth]
type = "direct"
pg_user = "PG_USER"
pg_pass_env = "PG_PASS"

[[clusters]]
name = "staging"
endpoint = "mcp-staging.atscaleinternal.com:15432"
required = false
priority = 2
supported_backends = ["sql", "dax"]

[clusters.auth]
type = "oidc"
token_url = "https://auth.example.com/token"
client_id = "my-client"
realm = "atscale"
client_secret_env = "OIDC_SECRET"
```

## Dependencies

- [mcp-cluster-registry](https://github.com/joeyen-atscale/mcp-cluster-registry) — typed cluster registry

## License

MIT OR Apache-2.0
