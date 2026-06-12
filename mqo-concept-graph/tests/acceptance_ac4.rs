/// AC4: Graph traversal — given a node id, neighbors(id, EdgeKind) returns only
/// edges of the specified kind, with correct source and target node ids.
use mqo_concept_graph::{ConceptGraph, EdgeKind};

const FIXTURE: &str = r#"{
  "measures": [
    {"unique_name": "m1", "name": "Sales"},
    {"unique_name": "m2", "name": "Costs"}
  ],
  "dimensions": [
    {
      "unique_name": "d1",
      "name": "Region",
      "hierarchies": [
        {
          "unique_name": "h1",
          "name": "Geo",
          "levels": [
            {"unique_name": "l_country", "name": "Country"},
            {"unique_name": "l_state",   "name": "State"},
            {"unique_name": "l_city",    "name": "City"}
          ]
        }
      ]
    },
    {
      "unique_name": "d2",
      "name": "Time",
      "hierarchies": [
        {
          "unique_name": "h2",
          "name": "Calendar",
          "levels": [
            {"unique_name": "l_year",  "name": "Year"},
            {"unique_name": "l_month", "name": "Month"}
          ]
        }
      ]
    }
  ],
  "calculated_members": [
    {"unique_name": "calc1", "name": "Net Sales"}
  ],
  "edges": [
    {"from": "m1", "to": "l_country", "kind": "aggregates_via"},
    {"from": "m1", "to": "l_year",    "kind": "aggregates_via"},
    {"from": "m1", "to": "m2",        "kind": "related_to"},
    {"from": "calc1", "to": "m1",     "kind": "derives_from"},
    {"from": "calc1", "to": "m2",     "kind": "derives_from"}
  ]
}"#;

#[test]
fn ac4_neighbors_filtered_by_aggregates_via() {
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();

    let agg_neighbors: Vec<&str> = graph
        .neighbors("m1", EdgeKind::AggregatesVia)
        .map(|n| n.unique_name.as_str())
        .collect();

    // Should contain l_country and l_year, but NOT m2 (which is related_to)
    assert!(
        agg_neighbors.contains(&"l_country"),
        "m1 should aggregate_via l_country; got: {agg_neighbors:?}"
    );
    assert!(
        agg_neighbors.contains(&"l_year"),
        "m1 should aggregate_via l_year; got: {agg_neighbors:?}"
    );
    assert!(
        !agg_neighbors.contains(&"m2"),
        "m2 should NOT appear in AggregatesVia neighbors of m1"
    );
    assert_eq!(
        agg_neighbors.len(),
        2,
        "exactly 2 AggregatesVia neighbors expected"
    );
}

#[test]
fn ac4_neighbors_filtered_by_related_to() {
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();

    let related_neighbors: Vec<&str> = graph
        .neighbors("m1", EdgeKind::RelatedTo)
        .map(|n| n.unique_name.as_str())
        .collect();

    assert_eq!(
        related_neighbors,
        vec!["m2"],
        "RelatedTo neighbors of m1 should be exactly [m2]"
    );
}

#[test]
fn ac4_neighbors_filtered_by_derives_from() {
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();

    let derives_neighbors: Vec<&str> = graph
        .neighbors("calc1", EdgeKind::DerivesFrom)
        .map(|n| n.unique_name.as_str())
        .collect();

    assert!(
        derives_neighbors.contains(&"m1"),
        "calc1 should derive_from m1"
    );
    assert!(
        derives_neighbors.contains(&"m2"),
        "calc1 should derive_from m2"
    );
    assert_eq!(derives_neighbors.len(), 2);
}

#[test]
fn ac4_neighbors_returns_empty_for_wrong_kind() {
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();

    // m1 has no TimeShifts or FiltersBy edges
    let ts_neighbors: Vec<&str> = graph
        .neighbors("m1", EdgeKind::TimeShifts)
        .map(|n| n.unique_name.as_str())
        .collect();
    assert!(ts_neighbors.is_empty(), "m1 should have no TimeShifts edges");

    let fb_neighbors: Vec<&str> = graph
        .neighbors("m1", EdgeKind::FiltersBy)
        .map(|n| n.unique_name.as_str())
        .collect();
    assert!(fb_neighbors.is_empty(), "m1 should have no FiltersBy edges");
}

#[test]
fn ac4_neighbors_returns_empty_for_unknown_node() {
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();

    let neighbors: Vec<_> = graph
        .neighbors("nonexistent_node_xyz", EdgeKind::AggregatesVia)
        .collect();
    assert!(
        neighbors.is_empty(),
        "unknown node should return empty neighbor iterator"
    );
}

#[test]
fn ac4_hierarchy_contains_exactly_its_levels_via_related_to() {
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();

    let h1_levels: Vec<&str> = graph
        .neighbors("h1", EdgeKind::RelatedTo)
        .map(|n| n.unique_name.as_str())
        .collect();

    assert!(h1_levels.contains(&"l_country"), "h1 should relate to l_country");
    assert!(h1_levels.contains(&"l_state"), "h1 should relate to l_state");
    assert!(h1_levels.contains(&"l_city"), "h1 should relate to l_city");
    assert_eq!(h1_levels.len(), 3, "h1 should have exactly 3 levels");

    // h2 should not appear among h1's neighbors
    assert!(
        !h1_levels.contains(&"h2"),
        "h2 should not be a neighbor of h1"
    );
}
