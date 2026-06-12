//! AC2: DatasetSummary never contains the full dataset.
//! Constructs a 10_000-row logical dataset; asserts sample.len() ≤ sample_cap
//! and total serialized size < a fixed KB cap.

use dh_spec::{ColumnRole, ColumnSchema, DatasetSummary, DType, Row, DEFAULT_SAMPLE_CAP};
use serde_json::Value;
use std::collections::HashMap;

/// Total serialized-summary size must stay below this limit (bytes).
const KB_CAP: usize = 64 * 1024; // 64 KiB — generous but finite

fn make_10k_rows() -> Vec<Row> {
    (0u64..10_000)
        .map(|i| {
            let mut row = HashMap::new();
            row.insert("id".to_string(), Value::from(i));
            row.insert("revenue".to_string(), Value::from(i as f64 * 1.5));
            row.insert(
                "region".to_string(),
                Value::from(format!("region_{}", i % 50)),
            );
            row
        })
        .collect()
}

#[test]
fn ac2_sample_capped_at_default_sample_cap() {
    let rows = make_10k_rows();
    let summary = DatasetSummary::new(
        10_000,
        vec![ColumnSchema {
            name: "revenue".to_string(),
            unique_name: "model.revenue".to_string(),
            dtype: DType::Float,
            nullable: false,
            role: ColumnRole::Measure,
        }],
        rows,
        DEFAULT_SAMPLE_CAP,
        HashMap::new(),
        vec![],
    );

    assert!(
        summary.sample.len() <= DEFAULT_SAMPLE_CAP,
        "sample.len()={} must be ≤ DEFAULT_SAMPLE_CAP={}",
        summary.sample.len(),
        DEFAULT_SAMPLE_CAP,
    );
    assert_eq!(summary.row_count, 10_000, "row_count must reflect full dataset");
}

#[test]
fn ac2_serialized_size_under_kb_cap() {
    let rows = make_10k_rows();
    let summary = DatasetSummary::new(
        10_000,
        vec![ColumnSchema {
            name: "revenue".to_string(),
            unique_name: "model.revenue".to_string(),
            dtype: DType::Float,
            nullable: false,
            role: ColumnRole::Measure,
        }],
        rows,
        DEFAULT_SAMPLE_CAP,
        HashMap::new(),
        vec![],
    );

    let serialized = serde_json::to_string(&summary).expect("serialize summary");
    assert!(
        serialized.len() < KB_CAP,
        "serialized summary size {} bytes exceeds KB cap {} bytes",
        serialized.len(),
        KB_CAP,
    );
}

#[test]
fn ac2_truncation_note_added_when_rows_exceed_cap() {
    let rows = make_10k_rows();
    let summary = DatasetSummary::new(10_000, vec![], rows, 5, HashMap::new(), vec![]);
    assert!(
        !summary.notes.is_empty(),
        "a truncation note must be added when rows exceed sample_cap"
    );
    let note_mentions_truncation = summary
        .notes
        .iter()
        .any(|n| n.contains("truncated") || n.contains("truncate"));
    assert!(note_mentions_truncation, "truncation note must mention 'truncated'");
}

#[test]
fn ac2_no_truncation_note_when_rows_within_cap() {
    let rows: Vec<Row> = (0..5)
        .map(|i| {
            let mut r = HashMap::new();
            r.insert("id".to_string(), Value::from(i));
            r
        })
        .collect();
    let summary = DatasetSummary::new(5, vec![], rows, 20, HashMap::new(), vec![]);
    let truncation_notes: Vec<_> = summary
        .notes
        .iter()
        .filter(|n| n.contains("truncated"))
        .collect();
    assert!(
        truncation_notes.is_empty(),
        "no truncation note expected when rows ≤ sample_cap"
    );
}

#[test]
fn ac2_custom_sample_cap_respected() {
    let rows = make_10k_rows();
    let cap = 7;
    let summary = DatasetSummary::new(10_000, vec![], rows, cap, HashMap::new(), vec![]);
    assert_eq!(summary.sample.len(), cap);
}
