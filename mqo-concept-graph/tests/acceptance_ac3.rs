/// AC3: from_describe_model returns Err (not panic) when given invalid or empty JSON.
use mqo_concept_graph::ConceptGraph;

#[test]
fn ac3_empty_string_returns_err() {
    assert!(
        ConceptGraph::from_describe_model("").is_err(),
        "empty string should return Err"
    );
}

#[test]
fn ac3_whitespace_only_returns_err() {
    assert!(
        ConceptGraph::from_describe_model("   \t\n  ").is_err(),
        "whitespace-only string should return Err"
    );
}

#[test]
fn ac3_invalid_json_returns_err() {
    assert!(
        ConceptGraph::from_describe_model("{not valid json}").is_err(),
        "invalid JSON should return Err"
    );
}

#[test]
fn ac3_json_array_not_object_returns_err() {
    // Valid JSON but not the expected object structure
    assert!(
        ConceptGraph::from_describe_model("[1, 2, 3]").is_err(),
        "JSON array should return InvalidStructure Err"
    );
}

#[test]
fn ac3_truncated_json_returns_err() {
    assert!(
        ConceptGraph::from_describe_model(r#"{"measures": [{"unique_name":"#).is_err(),
        "truncated JSON should return Err"
    );
}

#[test]
fn ac3_null_json_returns_err() {
    // `null` is valid JSON but not an object
    assert!(
        ConceptGraph::from_describe_model("null").is_err(),
        "null JSON should return Err"
    );
}
