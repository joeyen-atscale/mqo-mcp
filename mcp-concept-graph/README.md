# mcp-concept-graph

Attributed property graph of the AtScale semantic model, derived from `describe_model` JSON.

## What it is

Every downstream Gen-7/Gen-8 tool re-extracts its own slice of the model topology from `describe_model` JSON:
`mcp-grounding-eval` builds a flat entity index, `mcp-session-state` builds an adjacency list,
`mcp-sml-gap-patcher` does edit-distance matching. This library builds that topology **once, as a
typed attributed property graph**. Every downstream tool imports this crate and traverses the graph
rather than re-parsing JSON.

## Node kinds

| Kind | Source |
|---|---|
| `Measure` | `measures[]` array |
| `Hierarchy` | `dimensions[].hierarchies[]` |
| `DimensionLevel` | `dimensions[].hierarchies[].levels[]` |
| `Calc` | `calculated_members[]` / `calcs[]` |
| `DateRole` | `date_roles[]` / `time_dims[]` |

## Edge kinds

| Kind | Derivation rule |
|---|---|
| `LevelOf` | level → hierarchy |
| `ParentOf` | parent level → child level (in `levels[]` order) |
| `DerivesFrom` | calc → measure(s) referenced in `[Measures].[X]` patterns |
| `TimeShifts` | date_role → associated measure |
| `FiltersBy` | measure → restricted dimension binding |
| `AggregatesVia` | measure → expression component |
| `RelatedTo` | bidirectional; measures/dimensions sharing a `folder` (weight 0.5) |

## Quick start

```rust
use mcp_concept_graph::ConceptGraph;

let json: serde_json::Value = /* your describe_model response */;
let graph = ConceptGraph::from_describe_model(&json)?;

// All nodes one hop from "Total Sales"
let neighbors = graph.k_hop_neighbors("total_sales", 1);

// Shortest path between two nodes
if let Some(path) = graph.shortest_path("calc_margin", "lvl_year") {
    println!("{:?}", path);
}

// Induced subgraph over a selected set of nodes
let sub = graph.subgraph(&["rev", "cost", "calc_margin"]);

// JSON round-trip for caching / file exchange
let serialized = graph.to_json();
let restored   = ConceptGraph::from_json(&serialized);
```

## API

```rust
// Construction
ConceptGraph::from_describe_model(json: &serde_json::Value) -> Result<Self, GraphError>
ConceptGraph::from_describe_model_str(s: &str) -> Result<Self, GraphError>
ConceptGraph::from_json(json: &serde_json::Value) -> Self

// Accessors
graph.node(id)              -> Option<&Node>
graph.edges_from(id)        -> &[Edge]
graph.edges_to(id)          -> &[Edge]
graph.nodes()               -> Vec<&Node>
graph.edges()               -> Vec<&Edge>
graph.nodes_by_kind(kind)   -> Vec<&Node>
graph.neighbors(id, kind)   -> Vec<&Node>

// Traversal
graph.k_hop_neighbors(id, k: u8)     -> Vec<&Node>   // BFS; excludes source
graph.shortest_path(from, to)        -> Option<Vec<String>>  // BFS, unweighted

// Derived graphs
graph.subgraph(ids: &[&str])         -> ConceptGraph  // induced subgraph

// Serialization
graph.to_json()                      -> serde_json::Value
```

## Performance (AC7)

On a 500-measure, 200-level, 100-calc model:

- `from_describe_model`: < 200 ms
- `k_hop_neighbors` at k=3: < 50 ms

## Dependencies

`serde`, `serde_json`, `thiserror`. No network; no external graph DB; no subprocesses.

## License

MIT OR Apache-2.0
