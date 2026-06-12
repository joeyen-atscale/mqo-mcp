// AC1: from_describe_model on a model with 3 measures, 2 hierarchies
// (each with 3 levels), and 1 calc produces the correct node count
// and at least one LevelOf edge per level.

use mcp_concept_graph::{ConceptGraph, EdgeKind, NodeKind};
use serde_json::json;

fn sample_model() -> serde_json::Value {
    json!({
        "name": "test_model",
        "measures": [
            { "unique_name": "m1", "name": "Revenue" },
            { "unique_name": "m2", "name": "Cost" },
            { "unique_name": "m3", "name": "Profit" }
        ],
        "dimensions": [
            {
                "unique_name": "dim_date",
                "name": "Date",
                "hierarchies": [
                    {
                        "unique_name": "hier_date",
                        "name": "Date Hierarchy",
                        "levels": [
                            { "unique_name": "lvl_year",    "name": "Year" },
                            { "unique_name": "lvl_quarter", "name": "Quarter" },
                            { "unique_name": "lvl_month",   "name": "Month" }
                        ]
                    }
                ]
            },
            {
                "unique_name": "dim_geo",
                "name": "Geography",
                "hierarchies": [
                    {
                        "unique_name": "hier_geo",
                        "name": "Geo Hierarchy",
                        "levels": [
                            { "unique_name": "lvl_country", "name": "Country" },
                            { "unique_name": "lvl_state",   "name": "State" },
                            { "unique_name": "lvl_city",    "name": "City" }
                        ]
                    }
                ]
            }
        ],
        "calculated_members": [
            {
                "unique_name": "calc1",
                "name": "Margin",
                "expression": "([Measures].[Revenue] - [Measures].[Cost]) / [Measures].[Revenue]"
            }
        ]
    })
}

#[test]
fn test_node_count() {
    let g = ConceptGraph::from_describe_model(&sample_model()).unwrap();
    // 3 measures + 2 hierarchies + 6 levels + 1 calc = 12 nodes
    assert_eq!(g.nodes().len(), 12, "expected 12 nodes, got {}", g.nodes().len());
}

#[test]
fn test_level_of_edges_present() {
    let g = ConceptGraph::from_describe_model(&sample_model()).unwrap();
    let level_of_edges: Vec<_> = g
        .edges()
        .into_iter()
        .filter(|e| e.kind == EdgeKind::LevelOf)
        .collect();
    // 6 levels → 6 LevelOf edges
    assert_eq!(
        level_of_edges.len(),
        6,
        "expected 6 LevelOf edges, got {}",
        level_of_edges.len()
    );
}

#[test]
fn test_node_kinds() {
    let g = ConceptGraph::from_describe_model(&sample_model()).unwrap();
    assert_eq!(g.nodes_by_kind(NodeKind::Measure).len(), 3);
    assert_eq!(g.nodes_by_kind(NodeKind::Hierarchy).len(), 2);
    assert_eq!(g.nodes_by_kind(NodeKind::DimensionLevel).len(), 6);
    assert_eq!(g.nodes_by_kind(NodeKind::Calc).len(), 1);
}

#[test]
fn test_parent_of_edges() {
    let g = ConceptGraph::from_describe_model(&sample_model()).unwrap();
    let parent_edges: Vec<_> = g
        .edges()
        .into_iter()
        .filter(|e| e.kind == EdgeKind::ParentOf)
        .collect();
    // 2 levels per hierarchy have a parent (Year→Quarter, Quarter→Month for date; same for geo)
    // 2 hierarchies × 2 ParentOf = 4
    assert_eq!(
        parent_edges.len(),
        4,
        "expected 4 ParentOf edges, got {}",
        parent_edges.len()
    );
}
