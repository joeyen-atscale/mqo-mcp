# mcp-cross-cluster-diff

Diff two AtScale cluster `describe_model` catalogs and classify divergences.
Answers "how is prod different from staging?" for model promotion validation.

## Usage

```
mcp-cross-cluster-diff \
  --catalog-a  catalog-prod.json    \
  --catalog-b  catalog-staging.json \
  --cluster-a  prod                 \
  --cluster-b  staging              \
  [--numeric-tolerance 0.001]       \
  [--format json|human]             \
  [--output diff.json]
```

## What it diffs

For every measure and dimension (identified by `unique_name`) across all models:

| Scenario | Classification |
|---|---|
| Entity in A, not in B | `only_in_a` |
| Entity in B, not in A | `only_in_b` |
| All fields match | `agree` |
| Non-critical field differs (name, folder, format_string) | `diverge` |
| Semantic field differs (expression, aggregation_type) | `critical_diverge` |

## Output schema

```json
{
  "clusters": { "a": "prod", "b": "staging" },
  "summary": {
    "agree": 3,
    "diverge": 1,
    "critical_diverge": 0,
    "only_in_a": 0,
    "only_in_b": 0
  },
  "differences": [
    {
      "entity_type": "measure",
      "unique_name": "Total Store Sales",
      "verdict": "diverge",
      "field_diffs": [
        {
          "field": "folder",
          "cluster_a": "Sales",
          "cluster_b": "Marketing",
          "critical": false
        }
      ]
    }
  ],
  "only_in_a": [],
  "only_in_b": [],
  "overall_verdict": "diverge"
}
```

## Exit codes

| Code | Meaning |
|---|---|
| 0 | All entities agree |
| 1 | At least one diverge (non-critical field difference or missing entity) |
| 2 | At least one critical_diverge (expression or aggregation_type differs) |

## Human-readable output

Pass `--format human` for a compact text report:

```
=== mcp-cross-cluster-diff ===
Cluster A : prod
Cluster B : staging
Verdict   : DIVERGE

Summary:
  agree            : 3
  diverge          : 1
  critical_diverge : 0
  only_in_a        : 0
  only_in_b        : 0

Differences:
  [measure] Total Store Sales => Diverge
    .folder: A="Sales" B="Marketing"
```

## Dependencies

- `mcp-cluster-registry` (path dep) — cluster configuration types
- `serde` / `serde_json` — JSON parsing
- `clap` — CLI argument parsing

## Relationship to other tools

| Tool | What it diffs |
|---|---|
| `mcp-cross-cluster-diff` | Catalog schema (measures, dimensions) between two clusters |
| `sql-structural-diff` | SQL query structure (AST-level) |
| Tiger regression tracker | Query results: build N vs build N-1 on same cluster |
