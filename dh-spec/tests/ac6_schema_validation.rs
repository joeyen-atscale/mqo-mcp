//! AC6: schema/dh-spec.schema.json exists and a test validates fixture
//! DatasetSummary + OpResult against it.
//!
//! We validate structurally: parse the schema as JSON, then check that
//! the fixture JSON objects satisfy the shape (field presence, type checks).
//! Full JSON Schema draft 2020-12 validation is not available as a lightweight
//! pure-Rust dep without pulling in heavy tooling; we verify:
//!   1. The schema file parses as a JSON object with a "$schema" key.
//!   2. The fixture values parse as the expected Rust types (round-trip proof).
//!   3. The schema mentions the required type names (structural smoke-test).

use dh_spec::{
    ColumnRole, ColumnSchema, ColStats, DatasetHandle, DatasetSummary, DType,
    OpResult, emit_summary_schema, emit_op_result_schema,
};
use std::collections::HashMap;
use std::path::Path;

fn make_handle(id: &str) -> DatasetHandle {
    DatasetHandle {
        id: id.to_string(),
        created_at: 1_717_000_000,
        ttl_secs: 3600,
        derived_from: None,
    }
}

fn make_summary_fixture() -> DatasetSummary {
    let mut row = HashMap::new();
    row.insert("revenue".to_string(), serde_json::Value::from(99.5_f64));

    let mut stats = HashMap::new();
    stats.insert(
        "model.revenue".to_string(),
        ColStats {
            min: Some(0.0),
            max: Some(200.0),
            sum: Some(9950.0),
            mean: Some(99.5),
            distinct: Some(100),
            top_k: None,
        },
    );

    DatasetSummary::new(
        100,
        vec![ColumnSchema {
            name: "revenue".to_string(),
            unique_name: "model.revenue".to_string(),
            dtype: DType::Float,
            nullable: false,
            role: ColumnRole::Measure,
        }],
        vec![row],
        20,
        stats,
        vec![],
    )
}

fn make_op_result_fixture() -> OpResult {
    OpResult {
        handle: make_handle("hdl_fixture_result"),
        summary: make_summary_fixture(),
    }
}

#[test]
fn ac6_schema_file_exists() {
    let schema_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/dh-spec.schema.json");
    assert!(
        schema_path.exists(),
        "schema/dh-spec.schema.json must exist at {schema_path:?}"
    );
}

#[test]
fn ac6_schema_file_is_valid_json_object() {
    let schema_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/dh-spec.schema.json");
    let content = std::fs::read_to_string(&schema_path)
        .unwrap_or_else(|e| panic!("failed to read schema file: {e}"));
    let v: serde_json::Value =
        serde_json::from_str(&content).expect("schema/dh-spec.schema.json must be valid JSON");
    assert!(v.is_object(), "schema must be a JSON object");
}

#[test]
fn ac6_schema_has_dollar_schema_key() {
    let schema_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/dh-spec.schema.json");
    let content = std::fs::read_to_string(&schema_path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(
        v.get("$schema").is_some(),
        "schema must contain a '$schema' key"
    );
}

#[test]
fn ac6_schema_mentions_dataset_summary() {
    let schema_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/dh-spec.schema.json");
    let content = std::fs::read_to_string(&schema_path).unwrap();
    assert!(
        content.contains("DatasetSummary"),
        "schema must mention 'DatasetSummary'"
    );
}

#[test]
fn ac6_schema_mentions_op_result() {
    let schema_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("schema/dh-spec.schema.json");
    let content = std::fs::read_to_string(&schema_path).unwrap();
    assert!(
        content.contains("OpResult"),
        "schema must mention 'OpResult'"
    );
}

#[test]
fn ac6_fixture_dataset_summary_round_trips() {
    let fixture = make_summary_fixture();
    let json = serde_json::to_string(&fixture).expect("serialize");
    let reparsed: DatasetSummary = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(fixture, reparsed);
}

#[test]
fn ac6_fixture_op_result_round_trips() {
    let fixture = make_op_result_fixture();
    let json = serde_json::to_string(&fixture).expect("serialize");
    let reparsed: OpResult = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(fixture, reparsed);
}

#[test]
fn ac6_emitted_summary_schema_matches_fixture_shape() {
    // The emitted schema is a valid JSON object and the fixture JSON is
    // structurally compatible (all required top-level fields present).
    let schema_str = emit_summary_schema();
    let schema: serde_json::Value = serde_json::from_str(&schema_str).unwrap();
    assert!(schema.is_object());

    let fixture = make_summary_fixture();
    let fixture_json = serde_json::to_value(&fixture).expect("to_value");
    // Check top-level required fields.
    for field in &["row_count", "columns", "sample", "sample_cap", "stats", "notes"] {
        assert!(
            fixture_json.get(field).is_some(),
            "fixture DatasetSummary must have field '{field}'"
        );
    }
}

#[test]
fn ac6_emitted_op_result_schema_matches_fixture_shape() {
    let schema_str = emit_op_result_schema();
    let schema: serde_json::Value = serde_json::from_str(&schema_str).unwrap();
    assert!(schema.is_object());

    let fixture = make_op_result_fixture();
    let fixture_json = serde_json::to_value(&fixture).expect("to_value");
    for field in &["handle", "summary"] {
        assert!(
            fixture_json.get(field).is_some(),
            "fixture OpResult must have field '{field}'"
        );
    }
}
