//! AC4: OpResult.handle != OpRequest.handle is representable;
//! a test (and doc-test in lib.rs) shows the derive-new-handle pattern.

use dh_spec::{Capability, DatasetHandle, DatasetSummary, OpRequest, OpResult};
use std::collections::HashMap;

fn make_handle(id: &str) -> DatasetHandle {
    DatasetHandle {
        id: id.to_string(),
        created_at: 1_717_000_000,
        ttl_secs: 3600,
        derived_from: None,
    }
}

/// Simulate a server-side operation: takes an input handle, applies an op,
/// and mints a new output handle.  This is the canonical derive-new-handle
/// pattern.
fn simulate_op(req: OpRequest) -> OpResult {
    // Server mints a new id; the input handle id influences the derived name
    // only for illustration purposes.
    let new_id = format!("{}_derived", req.handle.id);
    let new_handle = DatasetHandle {
        id: new_id,
        created_at: req.handle.created_at + 1,
        ttl_secs: req.handle.ttl_secs,
        derived_from: Some(Box::new(req.handle)),
    };
    let summary = DatasetSummary::new(
        0,
        vec![],
        vec![],
        20,
        HashMap::new(),
        vec!["empty result for test".to_string()],
    );
    OpResult {
        handle: new_handle,
        summary,
    }
}

#[test]
fn ac4_op_result_handle_differs_from_request_handle() {
    let input_handle = make_handle("hdl_input");
    let req = OpRequest {
        handle: input_handle.clone(),
        op: Capability::Aggregate,
        params: serde_json::json!({"group_by": ["region"], "agg": "SUM"}),
    };

    let result = simulate_op(req);

    // The result handle must be distinct from the input.
    assert_ne!(
        result.handle.id, input_handle.id,
        "OpResult.handle must be a NEW handle, not the same as OpRequest.handle"
    );
}

#[test]
fn ac4_derived_from_links_back_to_input() {
    let input_handle = make_handle("hdl_input_2");
    let req = OpRequest {
        handle: input_handle.clone(),
        op: Capability::Filter,
        params: serde_json::json!({"column": "revenue", "gt": 50}),
    };

    let result = simulate_op(req);

    // derived_from should point back to the original handle.
    let parent = result.handle.derived_from.as_deref().expect("derived_from must be Some");
    assert_eq!(parent.id, input_handle.id, "derived_from.id must match the input handle id");
}

#[test]
fn ac4_op_request_round_trips_json() {
    let req = OpRequest {
        handle: make_handle("hdl_rt"),
        op: Capability::Sort,
        params: serde_json::json!({"by": "revenue", "dir": "desc"}),
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let req2: OpRequest = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(req, req2);
}

#[test]
fn ac4_op_result_round_trips_json() {
    let input = make_handle("hdl_in");
    let req = OpRequest {
        handle: input,
        op: Capability::TopN,
        params: serde_json::json!({"n": 10, "by": "revenue"}),
    };
    let result = simulate_op(req);
    let json = serde_json::to_string(&result).expect("serialize");
    let result2: OpResult = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(result, result2);
}
