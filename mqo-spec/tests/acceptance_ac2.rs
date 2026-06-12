//! AC2: mqo-spec emits a valid JSON Schema document for Mqo.

#[test]
fn schema_is_valid_json_schema() {
    let schema_str = mqo_spec::emit_json_schema();

    // Must be valid JSON.
    let schema_val: serde_json::Value =
        serde_json::from_str(&schema_str).expect("emit_json_schema produced invalid JSON");

    // Must be an object with a "title" field (schemars always adds this for named types).
    assert!(
        schema_val.is_object(),
        "schema root must be a JSON object"
    );

    let obj = schema_val.as_object().unwrap();

    // Must have a "$schema" or "title" field — schemars 0.8 emits "title".
    assert!(
        obj.contains_key("title") || obj.contains_key("$schema"),
        "schema must have a title or $schema key; got keys: {:?}",
        obj.keys().collect::<Vec<_>>()
    );

    // Must reference Mqo type — title or definitions must mention it.
    let schema_text = schema_str.to_lowercase();
    assert!(
        schema_text.contains("mqo") || schema_text.contains("multidimensional"),
        "schema must reference Mqo type; got: {schema_str}"
    );

    // Must have "properties" or "$defs" at root (schemars 0.8 emits definitions).
    assert!(
        obj.contains_key("properties") || obj.contains_key("definitions"),
        "schema must have properties or definitions; got keys: {:?}",
        obj.keys().collect::<Vec<_>>()
    );
}

#[test]
fn schema_contains_required_fields() {
    let schema_str = mqo_spec::emit_json_schema();
    let schema_val: serde_json::Value = serde_json::from_str(&schema_str).unwrap();

    // The schema should describe required fields of Mqo.
    let required_fields = ["model", "measures", "dimensions", "filters", "non_empty"];
    for field in &required_fields {
        assert!(
            schema_str.contains(field),
            "schema should mention field '{field}'"
        );
    }

    // The schema should mention TimeIntel variants.
    let time_intel_variants = ["yo_y", "prior_period", "to_date", "running_total", "share", "rank"];
    for variant in &time_intel_variants {
        assert!(
            schema_str.contains(variant),
            "schema should mention TimeIntel variant '{variant}'"
        );
    }

    let _ = schema_val; // silence unused warning
}
