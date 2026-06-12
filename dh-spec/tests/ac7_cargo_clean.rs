//! AC7: cargo test --release passes; cargo clippy --release -- -D warnings clean.
//!
//! This test file documents the acceptance criterion; the actual verification
//! is that this file compiles and the test suite passes under `cargo test --release`.
//! Clippy cleanliness is verified by running `cargo clippy --release -- -D warnings`
//! in CI / the build script.

use dh_spec::{
    ALL_CAPABILITIES, INLINE_THRESHOLD, DEFAULT_SAMPLE_CAP,
    DatasetHandle, ColumnSchema, ColumnRole, DType,
    DatasetSummary, Capability, OpRequest, OpResult, Lineage, ColStats,
    emit_summary_schema, emit_op_result_schema,
};
use std::collections::HashMap;

/// Smoke-test that all public symbols are importable and usable.
#[test]
fn ac7_all_public_symbols_accessible() {
    // Constants
    let _ = INLINE_THRESHOLD;
    let _ = DEFAULT_SAMPLE_CAP;
    let _ = ALL_CAPABILITIES;

    // DatasetHandle
    let h = DatasetHandle {
        id: "hdl_ac7".to_string(),
        created_at: 0,
        ttl_secs: 60,
        derived_from: None,
    };

    // ColumnSchema with every DType and ColumnRole variant
    let dtypes = [
        DType::Int, DType::Float, DType::Decimal,
        DType::Str, DType::Bool, DType::Date, DType::Time,
    ];
    let roles = [ColumnRole::Measure, ColumnRole::Dimension, ColumnRole::Derived];
    for (i, dtype) in dtypes.iter().enumerate() {
        let _ = ColumnSchema {
            name: format!("col{i}"),
            unique_name: format!("model.col{i}"),
            dtype: *dtype,
            nullable: i % 2 == 0,
            role: roles[i % 3],
        };
    }

    // DatasetSummary
    let summary = DatasetSummary::new(
        0,
        vec![],
        vec![],
        DEFAULT_SAMPLE_CAP,
        HashMap::new(),
        vec![],
    );

    // All nine Capability variants
    for cap in ALL_CAPABILITIES {
        let _ = cap;
    }

    // OpRequest
    let req = OpRequest {
        handle: h.clone(),
        op: Capability::Describe,
        params: serde_json::Value::Null,
    };

    // OpResult
    let _result = OpResult {
        handle: DatasetHandle {
            id: "hdl_new".to_string(),
            created_at: 1,
            ttl_secs: 60,
            derived_from: Some(Box::new(h.clone())),
        },
        summary,
    };

    // Lineage
    let _lineage = Lineage {
        handle: h.clone(),
        op: req.op,
        params: req.params,
        parents: vec![h],
    };

    // ColStats
    let _stats = ColStats {
        min: Some(0.0),
        max: Some(100.0),
        sum: Some(5000.0),
        mean: Some(50.0),
        distinct: Some(100),
        top_k: Some(vec![serde_json::json!("top"), serde_json::json!("k")]),
    };

    // Schema emission
    let schema_summary = emit_summary_schema();
    assert!(!schema_summary.is_empty());
    let schema_result = emit_op_result_schema();
    assert!(!schema_result.is_empty());
}
