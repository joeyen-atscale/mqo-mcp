//! AC6: Given a catalog with no `column_group` fields, `build_matrix` returns
//! an empty matrix with a diagnostic note — not an all-compatible matrix.

use mqoguard_compatibility_matrix::{build_matrix, EnrichedCatalog, EnrichedColumn, MatrixConfig};
use std::collections::BTreeSet;

fn catalog_no_column_groups() -> EnrichedCatalog {
    EnrichedCatalog {
        model: "test_ac6_no_groups".into(),
        columns: vec![
            // Measure with no column_group (empty set — as emitted when bindings are absent)
            EnrichedColumn {
                unique_name: "sales_amount".into(),
                label: "Sales Amount".into(),
                kind: "measure".into(),
                is_calc: false,
                hierarchy: None,
                level: None,
                column_group: BTreeSet::new(), // empty — no binding
            },
            EnrichedColumn {
                unique_name: "inv_qty".into(),
                label: "Inventory Qty".into(),
                kind: "measure".into(),
                is_calc: false,
                hierarchy: None,
                level: None,
                column_group: BTreeSet::new(),
            },
            EnrichedColumn {
                unique_name: "date_day".into(),
                label: "Date Day".into(),
                kind: "dimension".into(),
                is_calc: false,
                hierarchy: Some("DateHier".into()),
                level: Some("Day".into()),
                column_group: BTreeSet::new(), // empty — no binding
            },
        ],
    }
}

#[test]
fn acceptance_ac6_empty_column_groups_returns_empty_matrix() {
    let catalog = catalog_no_column_groups();
    let matrix = build_matrix(&catalog, &MatrixConfig::default());

    // Must return empty matrix, NOT all-compatible
    assert!(
        matrix.measures.is_empty(),
        "measures map must be empty when no column_groups are present (fail-safe)"
    );
    assert!(
        matrix.hierarchies.is_empty(),
        "hierarchies map must be empty when no column_groups are present"
    );

    // Must include a diagnostic note
    let note = matrix.note.as_deref().unwrap_or("");
    assert!(
        !note.is_empty(),
        "note must be present explaining no column_group data"
    );
    assert!(
        note.to_lowercase().contains("column_group")
            || note.to_lowercase().contains("enrichment"),
        "note must reference column_group absence; got: {note}"
    );
}

#[test]
fn acceptance_ac6_not_all_compatible() {
    // Paranoia check: verify we didn't accidentally build an all-compatible matrix
    let catalog = catalog_no_column_groups();
    let matrix = build_matrix(&catalog, &MatrixConfig::default());

    // An all-compatible matrix would have sales_amount listing DateHier
    // Correct behavior: measures is empty
    assert!(
        !matrix.measures.contains_key("sales_amount"),
        "sales_amount must NOT appear with an all-compatible compatible_hierarchies set"
    );
}
