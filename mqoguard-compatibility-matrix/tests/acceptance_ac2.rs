//! AC2: Given a conformed dimension hierarchy whose levels span {sales, inventory},
//! `build_matrix` lists it compatible with both a sales measure and an inventory
//! measure — no false exclusion.

use mqoguard_compatibility_matrix::{
    build_matrix, is_symmetric, EnrichedCatalog, EnrichedColumn, MatrixConfig,
};
use std::collections::BTreeSet;

fn make_catalog() -> EnrichedCatalog {
    EnrichedCatalog {
        model: "test_ac2".into(),
        columns: vec![
            // Sales measure
            EnrichedColumn {
                unique_name: "sales_amount".into(),
                label: "Sales Amount".into(),
                kind: "measure".into(),
                is_calc: false,
                hierarchy: None,
                level: None,
                column_group: BTreeSet::from(["sales".into()]),
            },
            // Inventory measure
            EnrichedColumn {
                unique_name: "inv_qty".into(),
                label: "Inventory Quantity".into(),
                kind: "measure".into(),
                is_calc: false,
                hierarchy: None,
                level: None,
                column_group: BTreeSet::from(["inventory".into()]),
            },
            // Conformed dimension: Product hierarchy spans both facts
            // Level 1 joins sales facts
            EnrichedColumn {
                unique_name: "product_category".into(),
                label: "Product Category".into(),
                kind: "dimension".into(),
                is_calc: false,
                hierarchy: Some("ProductHier".into()),
                level: Some("Category".into()),
                column_group: BTreeSet::from(["sales".into()]),
            },
            // Level 2 joins inventory facts
            EnrichedColumn {
                unique_name: "product_item".into(),
                label: "Product Item".into(),
                kind: "dimension".into(),
                is_calc: false,
                hierarchy: Some("ProductHier".into()),
                level: Some("Item".into()),
                column_group: BTreeSet::from(["inventory".into()]),
            },
        ],
    }
}

#[test]
fn acceptance_ac2_conformed_dimension_compatible_with_both_facts() {
    let catalog = make_catalog();
    let matrix = build_matrix(&catalog, &MatrixConfig::default());

    // Sales measure must be compatible with the conformed ProductHier
    let sales_mc = matrix.measures.get("sales_amount");
    assert!(sales_mc.is_some(), "sales_amount must appear in matrix");
    let sales_compat_product = sales_mc
        .is_some_and(|m| m.compatible_hierarchies.contains("ProductHier"));
    assert!(
        sales_compat_product,
        "sales_amount should be compatible with conformed ProductHier (which has sales levels)"
    );

    // Inventory measure must also be compatible with the conformed ProductHier
    let inv_mc = matrix.measures.get("inv_qty");
    assert!(inv_mc.is_some(), "inv_qty must appear in matrix");
    let inv_compat_product = inv_mc
        .is_some_and(|m| m.compatible_hierarchies.contains("ProductHier"));
    assert!(
        inv_compat_product,
        "inv_qty should be compatible with conformed ProductHier (which has inventory levels)"
    );

    // Inverse: ProductHier must list both measures
    let product_hc = matrix.hierarchies.get("ProductHier");
    assert!(product_hc.is_some(), "ProductHier must appear in inverse index");
    let hier_has_sales = product_hc
        .is_some_and(|h| h.compatible_measures.contains("sales_amount"));
    let hier_has_inv = product_hc
        .is_some_and(|h| h.compatible_measures.contains("inv_qty"));
    assert!(
        hier_has_sales,
        "ProductHier inverse index must contain sales_amount"
    );
    assert!(
        hier_has_inv,
        "ProductHier inverse index must contain inv_qty"
    );

    // Symmetry
    assert!(is_symmetric(&matrix), "matrix must be symmetric");
}
