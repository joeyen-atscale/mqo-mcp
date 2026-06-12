//! AC5: All public API types implement serde Serialize/Deserialize. A round-trip
//! (serialize ConceptGraph to JSON, deserialize back) produces a graph with
//! identical node and edge counts.

use mqo_graph_traversal::{
    build_graph, causal_paths, related_measures, suggest_next_questions, CausalPath,
    ConceptGraphSnapshot, EdgeKind, EvidenceType, NextQuestion, PathStep, RelatedMeasure,
};

fn fixture_json() -> &'static str {
    r#"{
        "measures": [
            {"unique_name": "revenue", "name": "Revenue"},
            {"unique_name": "cost",    "name": "Cost"}
        ],
        "dimensions": [
            {"unique_name": "d_date", "name": "Date", "hierarchies": [
                {"unique_name": "h_cal", "name": "Calendar", "levels": [
                    {"unique_name": "l_year", "name": "Year"}
                ]}
            ]}
        ],
        "calculated_members": [
            {"unique_name": "profit", "name": "Profit",
             "formula_refs": [{"unique_name": "revenue"}, {"unique_name": "cost"}]}
        ],
        "edges": [
            {"from": "revenue", "to": "l_year", "kind": "aggregates_via"},
            {"from": "cost",    "to": "l_year", "kind": "aggregates_via"}
        ]
    }"#
}

#[test]
fn serde_roundtrip() {
    let graph = build_graph(fixture_json()).expect("fixture must be valid");

    let original_nodes = graph.node_count();
    let original_edges = graph.edge_count();

    // Serialize → JSON string
    let snap = graph.to_snapshot();
    let json_str = serde_json::to_string(&snap).expect("snapshot must serialize");

    // Deserialize back → ConceptGraphSnapshot → ConceptGraph
    let snap2: ConceptGraphSnapshot =
        serde_json::from_str(&json_str).expect("snapshot JSON must deserialize");
    let graph2 =
        mqo_graph_traversal::ConceptGraph::from_snapshot(snap2).expect("snapshot must rebuild graph");

    assert_eq!(
        graph2.node_count(),
        original_nodes,
        "node count must be identical after round-trip"
    );
    assert_eq!(
        graph2.edge_count(),
        original_edges,
        "edge count must be identical after round-trip"
    );

    // Verify derived types also serialize/deserialize
    let related: Vec<RelatedMeasure> = related_measures(&graph, "profit", 2);
    let related_json = serde_json::to_string(&related).expect("RelatedMeasure must serialize");
    let related2: Vec<RelatedMeasure> =
        serde_json::from_str(&related_json).expect("RelatedMeasure must deserialize");
    assert_eq!(related.len(), related2.len());

    let paths: Vec<CausalPath> = causal_paths(&graph, "profit");
    let paths_json = serde_json::to_string(&paths).expect("CausalPath must serialize");
    let paths2: Vec<CausalPath> =
        serde_json::from_str(&paths_json).expect("CausalPath must deserialize");
    assert_eq!(paths.len(), paths2.len());

    let questions: Vec<NextQuestion> = suggest_next_questions(&graph, &["revenue"]);
    let questions_json =
        serde_json::to_string(&questions).expect("NextQuestion must serialize");
    let questions2: Vec<NextQuestion> =
        serde_json::from_str(&questions_json).expect("NextQuestion must deserialize");
    assert_eq!(questions.len(), questions2.len());

    // Verify EdgeKind, EvidenceType, PathStep also round-trip
    let ek = EdgeKind::DerivesFrom;
    let ek_json = serde_json::to_string(&ek).unwrap();
    let ek2: EdgeKind = serde_json::from_str(&ek_json).unwrap();
    assert_eq!(ek, ek2);

    let et = EvidenceType::Structural;
    let et_json = serde_json::to_string(&et).unwrap();
    let et2: EvidenceType = serde_json::from_str(&et_json).unwrap();
    assert_eq!(et, et2);

    let step = PathStep {
        from: "a".to_owned(),
        to: "b".to_owned(),
        edge_kind: EdgeKind::FiltersBy,
    };
    let step_json = serde_json::to_string(&step).unwrap();
    let step2: PathStep = serde_json::from_str(&step_json).unwrap();
    assert_eq!(step, step2);
}
