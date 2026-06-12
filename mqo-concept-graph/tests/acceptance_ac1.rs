/// AC1: ConceptGraph::from_describe_model parses a describe_model JSON fixture
/// and returns a graph where node count matches the number of
/// measures + dimension levels + calcs + hierarchies in the fixture.
use mqo_concept_graph::ConceptGraph;

const FIXTURE: &str = r#"{
  "measures": [
    {"unique_name": "m1", "name": "Revenue"},
    {"unique_name": "m2", "name": "Cost"},
    {"unique_name": "m3", "name": "Units Sold"}
  ],
  "dimensions": [
    {
      "unique_name": "d1",
      "name": "Date",
      "hierarchies": [
        {
          "unique_name": "h1",
          "name": "Calendar",
          "levels": [
            {"unique_name": "l1", "name": "Year"},
            {"unique_name": "l2", "name": "Quarter"},
            {"unique_name": "l3", "name": "Month"}
          ]
        },
        {
          "unique_name": "h2",
          "name": "Fiscal",
          "levels": [
            {"unique_name": "l4", "name": "Fiscal Year"},
            {"unique_name": "l5", "name": "Fiscal Quarter"}
          ]
        }
      ]
    },
    {
      "unique_name": "d2",
      "name": "Product",
      "hierarchies": [
        {
          "unique_name": "h3",
          "name": "Product Category",
          "levels": [
            {"unique_name": "l6", "name": "Category"},
            {"unique_name": "l7", "name": "Subcategory"}
          ]
        }
      ]
    }
  ],
  "calculated_members": [
    {"unique_name": "c1", "name": "Gross Margin"},
    {"unique_name": "c2", "name": "Revenue YoY"}
  ],
  "edges": []
}"#;

#[test]
fn ac1_node_count_matches_fixture() {
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();

    // Expected: 3 measures + 2 hierarchies (d1) + 1 hierarchy (d2) + 5 levels (d1) + 2 levels (d2) + 2 calcs
    // = 3 + 3 + 7 + 2 = 15
    let measures = 3;
    let hierarchies = 3; // h1, h2, h3
    let levels = 7;      // l1..l7
    let calcs = 2;
    let expected_node_count = measures + hierarchies + levels + calcs;

    assert_eq!(
        graph.node_count(),
        expected_node_count,
        "expected {expected_node_count} nodes (measures + hierarchies + levels + calcs), got {}",
        graph.node_count()
    );
}

#[test]
fn ac1_all_unique_names_are_accessible() {
    let graph = ConceptGraph::from_describe_model(FIXTURE).unwrap();
    for uname in ["m1", "m2", "m3", "h1", "h2", "h3", "l1", "l2", "l3", "l4", "l5", "l6", "l7", "c1", "c2"] {
        assert!(
            graph.get_node(uname).is_some(),
            "node '{uname}' should be findable"
        );
    }
}
