/// AC2: Edges typed aggregates_via, time_shifts, filters_by, and derives_from
/// are present in the graph for fixtures that contain the corresponding semantic
/// relationships; each edge carries a typed enum variant, not a raw string.
use mqo_concept_graph::{ConceptGraph, EdgeKind};
use std::collections::HashSet;

const FIXTURE: &str = r#"{
  "measures": [
    {"unique_name": "m_revenue", "name": "Revenue"},
    {"unique_name": "m_revenue_yoy", "name": "Revenue YoY"},
    {"unique_name": "m_filtered", "name": "Filtered Revenue"}
  ],
  "dimensions": [
    {
      "unique_name": "d_date",
      "name": "Date",
      "hierarchies": [
        {
          "unique_name": "h_calendar",
          "name": "Calendar",
          "levels": [
            {"unique_name": "l_year", "name": "Year"},
            {"unique_name": "l_month", "name": "Month"}
          ]
        }
      ]
    }
  ],
  "calculated_members": [
    {"unique_name": "calc_gm", "name": "Gross Margin"}
  ],
  "edges": [
    {"from": "m_revenue",     "to": "l_year",       "kind": "aggregates_via"},
    {"from": "m_revenue",     "to": "l_month",      "kind": "aggregates_via"},
    {"from": "m_revenue_yoy", "to": "m_revenue",    "kind": "time_shifts"},
    {"from": "m_filtered",    "to": "l_year",       "kind": "filters_by"},
    {"from": "calc_gm",       "to": "m_revenue",    "kind": "derives_from"}
  ]
}"#;

#[test]
fn ac2_all_four_primary_edge_kinds_present() {
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();

    let kinds: HashSet<EdgeKind> = graph.edges().map(|(_, _, k)| k).collect();

    assert!(
        kinds.contains(&EdgeKind::AggregatesVia),
        "expected AggregatesVia edge to be present"
    );
    assert!(
        kinds.contains(&EdgeKind::TimeShifts),
        "expected TimeShifts edge to be present"
    );
    assert!(
        kinds.contains(&EdgeKind::FiltersBy),
        "expected FiltersBy edge to be present"
    );
    assert!(
        kinds.contains(&EdgeKind::DerivesFrom),
        "expected DerivesFrom edge to be present"
    );
}

#[test]
fn ac2_edges_are_typed_enum_variants() {
    // Verify that each edge has a specific EdgeKind variant (not a raw string).
    // We check this by exhaustive pattern matching — compile error if a string slips through.
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();

    for (_, _, kind) in graph.edges() {
        // This match exhausts all EdgeKind variants.  If the type were a String
        // or any other type the compiler would reject this code.
        match kind {
            EdgeKind::AggregatesVia => {}
            EdgeKind::TimeShifts => {}
            EdgeKind::FiltersBy => {}
            EdgeKind::DerivesFrom => {}
            EdgeKind::RelatedTo => {}
        }
    }
}

#[test]
fn ac2_aggregates_via_connects_measure_to_level() {
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();

    let agg_neighbors: Vec<&str> = graph
        .neighbors("m_revenue", EdgeKind::AggregatesVia)
        .map(|n| n.unique_name.as_str())
        .collect();

    assert!(
        agg_neighbors.contains(&"l_year"),
        "m_revenue should aggregate_via l_year"
    );
    assert!(
        agg_neighbors.contains(&"l_month"),
        "m_revenue should aggregate_via l_month"
    );
}

#[test]
fn ac2_time_shifts_edge_present() {
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();
    let ts_neighbors: Vec<&str> = graph
        .neighbors("m_revenue_yoy", EdgeKind::TimeShifts)
        .map(|n| n.unique_name.as_str())
        .collect();
    assert_eq!(ts_neighbors, vec!["m_revenue"]);
}

#[test]
fn ac2_filters_by_edge_present() {
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();
    let fb_neighbors: Vec<&str> = graph
        .neighbors("m_filtered", EdgeKind::FiltersBy)
        .map(|n| n.unique_name.as_str())
        .collect();
    assert_eq!(fb_neighbors, vec!["l_year"]);
}

#[test]
fn ac2_derives_from_edge_present() {
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();
    let df_neighbors: Vec<&str> = graph
        .neighbors("calc_gm", EdgeKind::DerivesFrom)
        .map(|n| n.unique_name.as_str())
        .collect();
    assert_eq!(df_neighbors, vec!["m_revenue"]);
}
