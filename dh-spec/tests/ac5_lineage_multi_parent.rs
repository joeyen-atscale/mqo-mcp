//! AC5: Lineage.parents can express a multi-parent op (e.g. compare);
//! a test builds a 2-parent lineage record.

use dh_spec::{Capability, DatasetHandle, Lineage};

fn make_handle(id: &str) -> DatasetHandle {
    DatasetHandle {
        id: id.to_string(),
        created_at: 1_717_000_000,
        ttl_secs: 3600,
        derived_from: None,
    }
}

#[test]
fn ac5_two_parent_lineage_for_compare_op() {
    let parent_a = make_handle("hdl_region_east");
    let parent_b = make_handle("hdl_region_west");
    let derived = make_handle("hdl_compare_east_vs_west");

    let lineage = Lineage {
        handle: derived,
        op: Capability::Compare,
        params: serde_json::json!({
            "metric": "revenue",
            "groups": ["East", "West"]
        }),
        parents: vec![parent_a.clone(), parent_b.clone()],
    };

    assert_eq!(lineage.parents.len(), 2, "Compare op must have 2 parents");
    assert_eq!(lineage.parents[0].id, parent_a.id);
    assert_eq!(lineage.parents[1].id, parent_b.id);
    assert_eq!(lineage.op, Capability::Compare);
}

#[test]
fn ac5_lineage_round_trips_json() {
    let lineage = Lineage {
        handle: make_handle("hdl_out"),
        op: Capability::Compare,
        params: serde_json::json!({"metric": "units"}),
        parents: vec![make_handle("hdl_p1"), make_handle("hdl_p2")],
    };
    let json = serde_json::to_string(&lineage).expect("serialize");
    let lineage2: Lineage = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(lineage, lineage2);
}

#[test]
fn ac5_lineage_single_parent_also_works() {
    let lineage = Lineage {
        handle: make_handle("hdl_filtered"),
        op: Capability::Filter,
        params: serde_json::json!({"col": "revenue", "gt": 100}),
        parents: vec![make_handle("hdl_base")],
    };
    assert_eq!(lineage.parents.len(), 1);
}

#[test]
fn ac5_lineage_zero_parents_allowed() {
    // A root lineage record (handle from a fresh query, no parents).
    let lineage = Lineage {
        handle: make_handle("hdl_root"),
        op: Capability::Aggregate,
        params: serde_json::json!({}),
        parents: vec![],
    };
    assert!(lineage.parents.is_empty());
}

#[test]
fn ac5_lineage_three_parent_join() {
    // Not in the PRD example, but Vec<DatasetHandle> must support it.
    let lineage = Lineage {
        handle: make_handle("hdl_join3"),
        op: Capability::Compare,
        params: serde_json::json!({"join": "union"}),
        parents: vec![
            make_handle("hdl_p1"),
            make_handle("hdl_p2"),
            make_handle("hdl_p3"),
        ],
    };
    assert_eq!(lineage.parents.len(), 3);
}
