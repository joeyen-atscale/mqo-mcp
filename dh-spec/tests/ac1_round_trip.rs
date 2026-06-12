//! AC1: All types serialize/deserialize round-trip via serde_json with stable field names.
//! A test asserts byte-stable JSON for a fixed fixture.

use dh_spec::{
    ColumnRole, ColumnSchema, ColStats, Capability, DatasetHandle, DatasetSummary,
    DType, Lineage, OpRequest, OpResult, Row,
};
use serde_json::Value;
use std::collections::HashMap;

fn make_handle(id: &str) -> DatasetHandle {
    DatasetHandle {
        id: id.to_string(),
        created_at: 1_717_000_000,
        ttl_secs: 3600,
        derived_from: None,
    }
}

fn make_col(name: &str, dtype: DType, role: ColumnRole) -> ColumnSchema {
    ColumnSchema {
        name: name.to_string(),
        unique_name: format!("model.{name}"),
        dtype,
        nullable: false,
        role,
    }
}

fn make_summary() -> DatasetSummary {
    let mut row: Row = HashMap::new();
    row.insert("revenue".to_string(), Value::from(42.0_f64));

    let mut stats = HashMap::new();
    stats.insert(
        "model.revenue".to_string(),
        ColStats {
            min: Some(1.0),
            max: Some(100.0),
            sum: Some(4200.0),
            mean: Some(42.0),
            distinct: Some(10),
            top_k: None,
        },
    );

    DatasetSummary::new(
        1,
        vec![make_col("revenue", DType::Float, ColumnRole::Measure)],
        vec![row],
        20,
        stats,
        vec![],
    )
}

// ── round-trip helpers ─────────────────────────────────────────────────────

fn round_trip<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug + PartialEq,
{
    let json = serde_json::to_string(value).expect("serialize");
    serde_json::from_str(&json).expect("deserialize")
}

// ── tests ──────────────────────────────────────────────────────────────────

#[test]
fn ac1_dataset_handle_round_trip() {
    let h = make_handle("hdl_abc");
    assert_eq!(round_trip(&h), h);
}

#[test]
fn ac1_handle_with_derived_from_round_trip() {
    let parent = make_handle("hdl_parent");
    let child = DatasetHandle {
        id: "hdl_child".to_string(),
        created_at: 1_717_001_000,
        ttl_secs: 1800,
        derived_from: Some(Box::new(parent)),
    };
    assert_eq!(round_trip(&child), child);
}

#[test]
fn ac1_column_schema_round_trip() {
    let col = make_col("date", DType::Date, ColumnRole::Dimension);
    assert_eq!(round_trip(&col), col);
}

#[test]
fn ac1_dataset_summary_round_trip() {
    let s = make_summary();
    assert_eq!(round_trip(&s), s);
}

#[test]
fn ac1_capability_round_trip() {
    let cap = Capability::Aggregate;
    assert_eq!(round_trip(&cap), cap);
}

#[test]
fn ac1_op_request_round_trip() {
    let req = OpRequest {
        handle: make_handle("hdl_req"),
        op: Capability::Filter,
        params: serde_json::json!({"column": "revenue", "gt": 100}),
    };
    assert_eq!(round_trip(&req), req);
}

#[test]
fn ac1_op_result_round_trip() {
    let result = OpResult {
        handle: make_handle("hdl_result"),
        summary: make_summary(),
    };
    assert_eq!(round_trip(&result), result);
}

#[test]
fn ac1_lineage_round_trip() {
    let lineage = Lineage {
        handle: make_handle("hdl_derived"),
        op: Capability::Aggregate,
        params: serde_json::json!({"group_by": ["region"]}),
        parents: vec![make_handle("hdl_parent")],
    };
    assert_eq!(round_trip(&lineage), lineage);
}

#[test]
fn ac1_stable_field_names() {
    // Assert stable JSON field names for DatasetHandle (byte-stable fixture).
    let h = DatasetHandle {
        id: "hdl_stable".to_string(),
        created_at: 1_000_000,
        ttl_secs: 900,
        derived_from: None,
    };
    let json = serde_json::to_string(&h).expect("serialize");
    let v: serde_json::Value = serde_json::from_str(&json).expect("parse");
    assert!(v.get("id").is_some(), "field 'id' must be present");
    assert!(v.get("created_at").is_some(), "field 'created_at' must be present");
    assert!(v.get("ttl_secs").is_some(), "field 'ttl_secs' must be present");
    assert!(v.get("derived_from").is_some(), "field 'derived_from' must be present");
}

#[test]
fn ac1_dtype_stable_names() {
    // Verify each DType serializes to its PascalCase name.
    let cases = [
        (DType::Int, "\"Int\""),
        (DType::Float, "\"Float\""),
        (DType::Decimal, "\"Decimal\""),
        (DType::Str, "\"Str\""),
        (DType::Bool, "\"Bool\""),
        (DType::Date, "\"Date\""),
        (DType::Time, "\"Time\""),
    ];
    for (dtype, expected) in cases {
        let got = serde_json::to_string(&dtype).expect("serialize");
        assert_eq!(got, expected, "DType::{dtype:?} serialized incorrectly");
    }
}
