//! AC5: When the configured payload budget is exceeded, the emitted matrix
//! drops the inverse index and retains the per-measure forward map.
//! The dropped index must be reconstructable from the output.

use mqoguard_compatibility_matrix::{
    build_matrix, EnrichedCatalog, EnrichedColumn, MatrixConfig,
};
use std::collections::{BTreeMap, BTreeSet};

fn make_catalog() -> EnrichedCatalog {
    EnrichedCatalog {
        model: "test_ac5".into(),
        columns: vec![
            EnrichedColumn {
                unique_name: "sales_amount".into(),
                label: "Sales Amount".into(),
                kind: "measure".into(),
                is_calc: false,
                hierarchy: None,
                level: None,
                column_group: BTreeSet::from(["sales".into()]),
            },
            EnrichedColumn {
                unique_name: "inv_qty".into(),
                label: "Inventory Qty".into(),
                kind: "measure".into(),
                is_calc: false,
                hierarchy: None,
                level: None,
                column_group: BTreeSet::from(["inventory".into()]),
            },
            EnrichedColumn {
                unique_name: "date_day".into(),
                label: "Date Day".into(),
                kind: "dimension".into(),
                is_calc: false,
                hierarchy: Some("DateHier".into()),
                level: Some("Day".into()),
                column_group: BTreeSet::from(["sales".into()]),
            },
            EnrichedColumn {
                unique_name: "inv_date".into(),
                label: "Inv Date".into(),
                kind: "dimension".into(),
                is_calc: false,
                hierarchy: Some("InvDateHier".into()),
                level: Some("Day".into()),
                column_group: BTreeSet::from(["inventory".into()]),
            },
        ],
    }
}

#[test]
fn acceptance_ac5_budget_exceeded_drops_inverse_index() {
    let catalog = make_catalog();
    let config = MatrixConfig {
        payload_budget_chars: 1, // tiny budget: forces drop
        include_inverse: true,
    };
    let matrix = build_matrix(&catalog, &config);

    // Forward map must be present
    assert!(
        !matrix.measures.is_empty(),
        "measures (forward map) must be retained even when budget exceeded"
    );

    // Inverse index must be dropped
    assert!(
        matrix.hierarchies.is_empty(),
        "hierarchies (inverse index) must be empty when budget exceeded"
    );

    // Note must explain the drop
    let note = matrix.note.as_deref().unwrap_or("");
    assert!(
        !note.is_empty(),
        "note must be present explaining the dropped inverse index"
    );
    assert!(
        note.to_lowercase().contains("reconstruct") || note.to_lowercase().contains("inverse"),
        "note must explain how to reconstruct the inverse index; got: {note}"
    );
}

#[test]
fn acceptance_ac5_inverse_reconstructable_from_forward_map() {
    let catalog = make_catalog();

    // First get the full matrix (with inverse)
    let full = build_matrix(&catalog, &MatrixConfig::default());

    // Then get the budget-exceeded matrix (without inverse)
    let config = MatrixConfig {
        payload_budget_chars: 1,
        include_inverse: true,
    };
    let trimmed = build_matrix(&catalog, &config);

    // Reconstruct inverse from trimmed's forward map
    let mut reconstructed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (measure_name, mc) in &trimmed.measures {
        for hier_id in &mc.compatible_hierarchies {
            reconstructed
                .entry(hier_id.clone())
                .or_default()
                .insert(measure_name.clone());
        }
    }

    // Reconstructed inverse must match the full matrix's inverse
    for (hier_id, hc) in &full.hierarchies {
        let rec = reconstructed.get(hier_id).cloned().unwrap_or_default();
        assert_eq!(
            &hc.compatible_measures,
            &rec,
            "reconstructed inverse for hierarchy {hier_id} must match the full inverse"
        );
    }
}
