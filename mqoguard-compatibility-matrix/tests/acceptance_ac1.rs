//! AC1: Given measure M (column-group `{sales}`) and hierarchy H with all levels
//! in column-group `{inventory}`, `build_matrix` excludes H from M's compatible set
//! and M from H's compatible set.

use mqoguard_compatibility_matrix::{
    build_matrix, is_symmetric, EnrichedCatalog, EnrichedColumn, MatrixConfig,
};
use std::collections::BTreeSet;

fn make_catalog() -> EnrichedCatalog {
    EnrichedCatalog {
        model: "test_ac1".into(),
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
            // Hierarchy H has all levels in {inventory} only
            EnrichedColumn {
                unique_name: "inv_level_1".into(),
                label: "Inventory Level 1".into(),
                kind: "dimension".into(),
                is_calc: false,
                hierarchy: Some("InventoryHier".into()),
                level: Some("Item".into()),
                column_group: BTreeSet::from(["inventory".into()]),
            },
            EnrichedColumn {
                unique_name: "inv_level_2".into(),
                label: "Inventory Level 2".into(),
                kind: "dimension".into(),
                is_calc: false,
                hierarchy: Some("InventoryHier".into()),
                level: Some("Warehouse".into()),
                column_group: BTreeSet::from(["inventory".into()]),
            },
        ],
    }
}

#[test]
fn acceptance_ac1_measure_excludes_incompatible_hierarchy() {
    let catalog = make_catalog();
    let matrix = build_matrix(&catalog, &MatrixConfig::default());

    // M's compatible set must NOT contain H
    let mc = matrix.measures.get("sales_amount");
    assert!(mc.is_some(), "sales_amount must appear in the matrix");
    let m_excludes_inv_hier = mc
        .is_some_and(|m| !m.compatible_hierarchies.contains("InventoryHier"));
    assert!(
        m_excludes_inv_hier,
        "sales_amount (sales group) should NOT be compatible with InventoryHier (inventory group)"
    );

    // H's compatible set must NOT contain M
    let hc = matrix.hierarchies.get("InventoryHier");
    assert!(hc.is_some(), "InventoryHier must appear in the inverse index");
    let h_excludes_sales = hc
        .is_some_and(|h| !h.compatible_measures.contains("sales_amount"));
    assert!(
        h_excludes_sales,
        "InventoryHier (inventory group) should NOT list sales_amount (sales group)"
    );

    // Symmetry must hold
    assert!(is_symmetric(&matrix), "matrix must be symmetric (AC3 invariant)");
}
