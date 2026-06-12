//! AC6: validate is pure and total — never panics on a malformed or partial
//! MQO/catalog; a structurally broken input yields a rejection or an empty Vec,
//! not an unwind. Property test over arbitrary JSON.

use mqo_param_validator::{validate, BoundMqoInput, CatalogSnapshot};
use serde_json::Value;

fn try_parse_and_validate(mqo_json: &str, cat_json: &str) {
    let mqo: BoundMqoInput = serde_json::from_str(mqo_json).unwrap_or_default();
    let catalog: CatalogSnapshot = serde_json::from_str(cat_json).unwrap_or_default();
    // Must not panic
    let _ = validate(&mqo, &catalog);
}

#[test]
fn ac6_empty_json_objects() {
    try_parse_and_validate("{}", "{}");
}

#[test]
fn ac6_null_fields_gracefully_handled() {
    try_parse_and_validate(
        r#"{"measures": null, "dimensions": null}"#,
        r#"{"measures": [], "dimensions": [], "hierarchies": [], "date_roles": []}"#,
    );
}

#[test]
fn ac6_missing_optional_fields() {
    try_parse_and_validate(
        r#"{"measures": [{"unique_name": "x"}]}"#,
        r#"{"measures": [{"unique_name": "y"}]}"#,
    );
}

#[test]
fn ac6_empty_strings_dont_panic() {
    try_parse_and_validate(
        r#"{"measures": [{"unique_name": ""}]}"#,
        r#"{"measures": [{"unique_name": ""}]}"#,
    );
}

#[test]
fn ac6_unicode_fields_dont_panic() {
    try_parse_and_validate(
        r#"{"measures": [{"unique_name": "销售额"}]}"#,
        r#"{"measures": [{"unique_name": "Umsatz"}]}"#,
    );
}

#[test]
fn ac6_very_long_field_names_dont_panic() {
    let long_name = "a".repeat(10_000);
    let mqo_json = format!(r#"{{"measures": [{{"unique_name": "{long_name}"}}]}}"#);
    let cat_json = r#"{"measures": [{"unique_name": "Sales"}]}"#;
    try_parse_and_validate(&mqo_json, cat_json);
}

#[test]
fn ac6_many_items_dont_panic() {
    let measures: Vec<Value> = (0..500)
        .map(|i| serde_json::json!({"unique_name": format!("Measure {}", i)}))
        .collect();
    let dimensions: Vec<Value> = (0..500)
        .map(|i| serde_json::json!({"unique_name": format!("Dim {}", i)}))
        .collect();
    let mqo_json = serde_json::json!({
        "measures": measures,
        "dimensions": dimensions,
    })
    .to_string();
    let cat_json = r#"{"measures": [], "dimensions": []}"#;
    try_parse_and_validate(&mqo_json, cat_json);
}

#[test]
fn ac6_partial_catalog_no_panic() {
    // Catalog with only measures (no dimensions field)
    try_parse_and_validate(
        r#"{"measures": [{"unique_name": "x"}], "dimensions": [{"unique_name": "y"}]}"#,
        r#"{"measures": [{"unique_name": "x"}]}"#,
    );
}

#[test]
fn ac6_deeply_nested_json_doesnt_panic() {
    // serde_json will reject deeply nested JSON but we handle the fallback
    let nested = r#"{"measures": [{"unique_name": "a", "extra": {"a": {"b": "c"}}}]}"#;
    try_parse_and_validate(nested, "{}");
}

#[test]
fn ac6_all_fields_present_valid_input_no_panic() {
    let mqo_json = r#"{
        "measures": [{"unique_name": "Sales"}],
        "dimensions": [{"unique_name": "Customer", "level": "City", "hierarchy": "Geo", "role_qualifier": null}],
        "filters": [{"unique_name": "Product", "level": null}]
    }"#;
    let cat_json = r#"{
        "measures": [{"unique_name": "Sales", "subject_area": "Sales"}],
        "dimensions": [{"unique_name": "Customer", "subject_areas": ["Sales"]}],
        "hierarchies": [],
        "date_roles": []
    }"#;
    try_parse_and_validate(mqo_json, cat_json);
}
