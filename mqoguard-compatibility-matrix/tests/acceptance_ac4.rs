//! AC4: Given the TPC-DS enriched catalog fixture:
//! - "Inventory Quantity On Hand" (`inv_quantity_on_hand`) is INCOMPATIBLE with
//!   the "Promotions" hierarchy (sales/promotions fact only).
//! - "Inventory Quantity On Hand" is COMPATIBLE with the "Inventory Calendar"
//!   hierarchy (inventory fact).

use mqoguard_compatibility_matrix::{
    build_matrix, is_symmetric, EnrichedCatalog, EnrichedColumn, MatrixConfig,
};
use std::collections::BTreeSet;

/// Minimal TPC-DS fixture capturing the two key relationships.
///
/// This is a simplified extract; the full catalog would come from the
/// enrichment pass over the live `AtScale` TPC-DS model.
fn tpcds_fixture() -> EnrichedCatalog {
    EnrichedCatalog {
        model: "postgres.tpcds.tpcds_benchmark_model".into(),
        columns: vec![
            // ── Inventory measure ──────────────────────────────────────────
            EnrichedColumn {
                unique_name: "inv_quantity_on_hand".into(),
                label: "Inventory Quantity On Hand".into(),
                kind: "measure".into(),
                is_calc: false,
                hierarchy: None,
                level: None,
                column_group: BTreeSet::from(["inventory".into()]),
            },
            // ── A sales measure (control) ──────────────────────────────────
            EnrichedColumn {
                unique_name: "ss_net_profit".into(),
                label: "SS Net Profit".into(),
                kind: "measure".into(),
                is_calc: false,
                hierarchy: None,
                level: None,
                column_group: BTreeSet::from(["sales".into()]),
            },
            // ── Promotions hierarchy (sales/promotions fact only) ──────────
            EnrichedColumn {
                unique_name: "promo_name".into(),
                label: "Promo Name".into(),
                kind: "dimension".into(),
                is_calc: false,
                hierarchy: Some("Promotions".into()),
                level: Some("Promo Name".into()),
                column_group: BTreeSet::from(["sales".into()]),
            },
            EnrichedColumn {
                unique_name: "promo_channel_tv".into(),
                label: "Promo Channel TV".into(),
                kind: "dimension".into(),
                is_calc: false,
                hierarchy: Some("Promotions".into()),
                level: Some("Channel".into()),
                column_group: BTreeSet::from(["sales".into()]),
            },
            // ── Inventory Calendar hierarchy (inventory fact) ──────────────
            EnrichedColumn {
                unique_name: "inv_date_sk".into(),
                label: "Inv Date SK".into(),
                kind: "dimension".into(),
                is_calc: false,
                hierarchy: Some("Inventory Calendar".into()),
                level: Some("Date".into()),
                column_group: BTreeSet::from(["inventory".into()]),
            },
            EnrichedColumn {
                unique_name: "inv_week".into(),
                label: "Inv Week".into(),
                kind: "dimension".into(),
                is_calc: false,
                hierarchy: Some("Inventory Calendar".into()),
                level: Some("Week".into()),
                column_group: BTreeSet::from(["inventory".into()]),
            },
        ],
    }
}

#[test]
fn acceptance_ac4_inventory_qty_incompatible_with_promotions() {
    let catalog = tpcds_fixture();
    let matrix = build_matrix(&catalog, &MatrixConfig::default());

    let mc = matrix.measures.get("inv_quantity_on_hand");
    assert!(mc.is_some(), "inv_quantity_on_hand must appear in matrix");
    let incompatible_with_promotions = mc
        .is_some_and(|m| !m.compatible_hierarchies.contains("Promotions"));
    assert!(
        incompatible_with_promotions,
        "Inventory Quantity On Hand (inventory fact) must NOT be compatible with Promotions (sales fact)"
    );
}

#[test]
fn acceptance_ac4_inventory_qty_compatible_with_inventory_calendar() {
    let catalog = tpcds_fixture();
    let matrix = build_matrix(&catalog, &MatrixConfig::default());

    let mc = matrix.measures.get("inv_quantity_on_hand");
    assert!(mc.is_some(), "inv_quantity_on_hand must appear in matrix");
    let compatible_with_inv_cal = mc
        .is_some_and(|m| m.compatible_hierarchies.contains("Inventory Calendar"));
    assert!(
        compatible_with_inv_cal,
        "Inventory Quantity On Hand (inventory fact) MUST be compatible with Inventory Calendar (inventory fact)"
    );

    // Also verify symmetry
    assert!(is_symmetric(&matrix), "matrix must be symmetric");
}
