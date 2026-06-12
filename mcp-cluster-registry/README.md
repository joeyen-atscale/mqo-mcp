# mcp-cluster-registry

Typed TOML/JSON multi-cluster configuration registry for the AtScale federation gateway.

## TL;DR

`mqo-auth-bridge`'s `LiveExecutor` holds exactly one endpoint + one auth config.
The federation gateway needs N of these, each with its own name, capabilities, and
priority. This library defines the typed TOML/JSON config schema for a multi-cluster
registry and provides the `ClusterRegistry` type that every downstream federation tool
consumes — the `mqo-spec` of the federation fleet.

## Usage

```toml
# Cargo.toml
[dependencies]
mcp-cluster-registry = { git = "https://github.com/joeyen-atscale/mcp-cluster-registry" }
```

```rust
use mcp_cluster_registry::ClusterRegistry;

let toml = std::fs::read_to_string("clusters.toml").unwrap();
let registry = ClusterRegistry::from_toml(&toml).unwrap();
registry.validate().unwrap();

// Route to the highest-priority required cluster
let primary = registry.primary_cluster().unwrap();

// Check backend support before routing
if registry.supports_backend("prod", "sql") {
    // ...
}

// Filter clusters by model
let clusters = registry.clusters_for_model("tpcds_benchmark_model");
```

## Example `clusters.toml`

```toml
[[clusters]]
name = "prod"
endpoint = "mcp-aws.atscaleinternal.com:15432"
supported_backends = ["sql"]
priority = 0
required = true
tags = ["prod", "snowflake"]

[clusters.auth]
type = "direct"
pg_user = "atscale_user"
pg_pass_env = "PROD_PG_PASS"

[[clusters]]
name = "staging"
endpoint = "mcp-staging.atscaleinternal.com:15432"
xmla_url = "http://mcp-staging.atscaleinternal.com:11111"
supported_backends = ["sql", "dax", "mdx"]
priority = 1
required = false
model_filter = ["tpcds_benchmark_model"]

[clusters.auth]
type = "oidc"
token_url = "https://mcp-staging.atscaleinternal.com/auth/realms/AtScale/protocol/openid-connect/token"
client_id = "atscale-mcp"
realm = "AtScale"
client_secret_env = "STAGING_OIDC_SECRET"
```

## Key design constraints

- **No credentials stored**: `AuthConfig` stores only env-var *names*. The serializer never calls `std::env::var()`. AC7 is enforced by tests.
- **No network, no async**: pure in-memory parsing and validation.
- **TOML for authoring, JSON for machine consumption**: `from_toml()` / `from_json()` / `to_json()` all produce the same in-memory type.

## API

```rust
// Parse
ClusterRegistry::from_toml(s: &str) -> Result<Self, RegistryError>
ClusterRegistry::from_json(s: &str) -> Result<Self, RegistryError>

// Serialize
ClusterRegistry::to_json(&self) -> String

// Query
ClusterRegistry::get(&self, name: &str) -> Option<&ClusterEntry>
ClusterRegistry::by_priority(&self) -> Vec<&ClusterEntry>
ClusterRegistry::supports_backend(&self, cluster: &str, backend: &str) -> bool
ClusterRegistry::clusters_for_model(&self, model_name: &str) -> Vec<&ClusterEntry>
ClusterRegistry::primary_cluster(&self) -> Option<&ClusterEntry>

// Validate
ClusterRegistry::validate(&self) -> Result<(), Vec<RegistryError>>
```

## License

MIT OR Apache-2.0
