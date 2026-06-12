//! AC5: chart op emits valid Vega-Lite JSON with $schema/mark/encoding.

use mqo_handle_walkthrough::ops;

#[path = "helpers.rs"]
mod helpers;

#[test]
fn ac5_vega_lite_has_required_fields() {
    let rows = helpers::load_fixture();
    let ca_rows = ops::slice_by_state(&rows, "California");
    let spec = ops::chart_vega_lite(&ca_rows, "Test Chart");

    // Must parse as JSON object.
    let obj = spec.as_object().expect("chart must be a JSON object");

    // $schema
    let schema_url = obj
        .get("$schema")
        .and_then(|v| v.as_str())
        .expect("must have $schema");
    assert!(
        schema_url.contains("vega-lite"),
        "$schema must reference vega-lite: {schema_url}"
    );

    // mark
    let mark = obj.get("mark").expect("must have mark");
    assert!(
        mark.as_str() == Some("line"),
        "mark must be 'line', got {mark:?}"
    );

    // encoding
    let encoding = obj
        .get("encoding")
        .and_then(|v| v.as_object())
        .expect("must have encoding object");

    let x = encoding.get("x").and_then(|v| v.as_object()).expect("encoding.x required");
    assert_eq!(
        x.get("field").and_then(|v| v.as_str()),
        Some("month"),
        "x.field must be 'month'"
    );

    let y = encoding.get("y").and_then(|v| v.as_object()).expect("encoding.y required");
    assert_eq!(
        y.get("field").and_then(|v| v.as_str()),
        Some("web_sales"),
        "y.field must be 'web_sales'"
    );
}

#[test]
fn ac5_chart_data_contains_rows() {
    let rows = helpers::load_fixture();
    let ca_rows = ops::slice_by_state(&rows, "California");
    let spec = ops::chart_vega_lite(&ca_rows, "Test Chart");

    let data_values = spec["data"]["values"]
        .as_array()
        .expect("data.values must be an array");
    assert_eq!(
        data_values.len(),
        ca_rows.len(),
        "data.values must contain all California rows"
    );
}
