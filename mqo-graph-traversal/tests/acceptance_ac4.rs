//! AC4: suggest_next_questions(graph, context_measures) returns at least one
//! NextQuestion (a measure node adjacent to context_measures but not in
//! context_measures) when the graph contains unexplored neighbors. Returns
//! empty vec when the context already covers all reachable nodes.

use mqo_graph_traversal::{build_graph, suggest_next_questions};

fn fixture_json() -> &'static str {
    r#"{
        "measures": [
            {"unique_name": "revenue",  "name": "Revenue"},
            {"unique_name": "cost",     "name": "Cost"},
            {"unique_name": "quantity", "name": "Quantity"}
        ],
        "dimensions": [],
        "calculated_members": [
            {"unique_name": "profit", "name": "Profit",
             "formula_refs": [{"unique_name": "revenue"}, {"unique_name": "cost"}]}
        ]
    }"#
}

#[test]
fn suggest_next_questions_neighbors() {
    let graph = build_graph(fixture_json()).expect("fixture must be valid");

    // Context = only "revenue"; profit is an unexplored neighbor (revenue → profit via DerivesFrom)
    let suggestions = suggest_next_questions(&graph, &["revenue"]);
    assert!(
        !suggestions.is_empty(),
        "with only 'revenue' in context, profit should be suggested"
    );
    let names: Vec<_> = suggestions.iter().map(|q| q.unique_name.as_str()).collect();
    assert!(
        names.contains(&"profit"),
        "profit should be suggested as a next question; got {names:?}"
    );
    // revenue itself must not appear in suggestions
    assert!(
        !names.contains(&"revenue"),
        "context nodes must not appear in suggestions"
    );

    // Context covers all measure+calc nodes → empty suggestions
    let full_context = suggest_next_questions(&graph, &["revenue", "cost", "quantity", "profit"]);
    assert!(
        full_context.is_empty(),
        "when context covers all reachable nodes, suggestions must be empty"
    );
}
