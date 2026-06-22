//! RULE 9 unit tests — NonLocalDimensionPath guard
//! (PRD-mqo-store-local-dimension-path-preference)
//!
//! AC1: generic-path pick flagged for a store measure.
//! AC2: store-local path passes (guard silent).
//! AC3: all-channel measure unaffected (FR5).
//! AC4: conformed-only dimension unaffected (FR4 — no fact-local sibling).
//! AC5: suggestion names the fact-local level (FR3 — one-step rebind).

use mqo_param_validator::{
    BoundMqoInput, CatalogDimension, CatalogHierarchy, CatalogMeasure, CatalogSnapshot,
    MqoDimensionRef, MqoMeasureRef, RejectReason, validate,
};

/// Catalog that mirrors the TPC-DS store-local brand scenario:
///   - `product_dimension.[Product Brand Name]`  — generic, conformed to all sales
///   - `store_item_product_dimension.[Store Item Product Brand Name]` — fact-local, store_sales only
///   - `Store Quantity Sold` — channel_scope: ["store_sales"]
///   - `Total Quantity Sold` — channel_scope: ["store_sales","catalog_sales","web_sales"]
fn brand_catalog() -> CatalogSnapshot {
    CatalogSnapshot {
        measures: vec![
            CatalogMeasure {
                unique_name: "tpcds.total_quantity_sold".into(),
                label: Some("Total Quantity Sold".into()),
                channel_scope: Some(vec![
                    "store_sales".into(),
                    "catalog_sales".into(),
                    "web_sales".into(),
                ]),
                ..Default::default()
            },
            CatalogMeasure {
                unique_name: "tpcds.store_quantity_sold".into(),
                label: Some("Store Quantity Sold".into()),
                channel_scope: Some(vec!["store_sales".into()]),
                ..Default::default()
            },
        ],
        dimensions: vec![
            CatalogDimension {
                unique_name: "product_dimension".into(),
                subject_areas: vec![],
            },
            CatalogDimension {
                unique_name: "store_item_product_dimension".into(),
                subject_areas: vec![],
            },
        ],
        hierarchies: vec![
            // Generic conformed brand hierarchy — available across all sales facts.
            CatalogHierarchy {
                dimension_unique_name: "product_dimension".into(),
                hierarchy_unique_name: "product_dimension".into(),
                levels: vec![
                    "Product Brand Name".into(),
                    "Product Category Name".into(),
                ],
                level_meta: vec![],
                fact_local_facts: vec![], // conformed — empty
            },
            // Fact-local brand hierarchy — bound only to store_sales + store_returns.
            CatalogHierarchy {
                dimension_unique_name: "store_item_product_dimension".into(),
                hierarchy_unique_name: "store_item_product_dimension".into(),
                levels: vec![
                    "Store Item Product Brand Name".into(),
                    "Store Item Product Category Name".into(),
                ],
                level_meta: vec![],
                fact_local_facts: vec!["store_sales".into(), "store_returns".into()],
            },
        ],
        date_roles: vec![],
    }
}

/// AC1: a fact-local (store) measure grouped by the generic brand level must be flagged.
#[test]
fn ac1_generic_brand_pick_flagged_for_store_measure() {
    let catalog = brand_catalog();
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "tpcds.store_quantity_sold".into(),
            aggregation: None,
        }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "product_dimension".into(),
            level: Some("Product Brand Name".into()),
            ..Default::default()
        }],
        filters: vec![],
    };
    let rejections = validate(&mqo, &catalog);
    let r9: Vec<_> = rejections
        .iter()
        .filter(|r| matches!(&r.reason, RejectReason::NonLocalDimensionPath { .. }))
        .collect();
    assert!(
        !r9.is_empty(),
        "AC1: RULE 9 must fire when store measure is grouped by generic brand level; got: {rejections:?}"
    );
    // AC5: suggestion must name the fact-local level.
    if let RejectReason::NonLocalDimensionPath {
        ref suggested_level,
        ref measure,
        ref fact,
        ref generic_level,
    } = r9[0].reason
    {
        assert!(
            suggested_level.contains("Store Item Product Brand Name"),
            "AC5: suggested_level must name the fact-local level, got: {suggested_level}"
        );
        assert!(
            generic_level.contains("Product Brand Name"),
            "AC5: generic_level should name the picked generic level, got: {generic_level}"
        );
        assert_eq!(
            fact, "store_sales",
            "AC5: fact must be the measure's channel scope, got: {fact}"
        );
        assert!(
            measure.contains("Store Quantity Sold"),
            "AC5: measure must name the store measure, got: {measure}"
        );
    }
}

/// AC2: a fact-local measure grouped by the fact-local brand level must pass (guard silent).
#[test]
fn ac2_fact_local_path_passes() {
    let catalog = brand_catalog();
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "tpcds.store_quantity_sold".into(),
            aggregation: None,
        }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "store_item_product_dimension".into(),
            level: Some("Store Item Product Brand Name".into()),
            ..Default::default()
        }],
        filters: vec![],
    };
    let rejections = validate(&mqo, &catalog);
    let r9: Vec<_> = rejections
        .iter()
        .filter(|r| matches!(&r.reason, RejectReason::NonLocalDimensionPath { .. }))
        .collect();
    assert!(
        r9.is_empty(),
        "AC2: RULE 9 must be silent when the agent correctly picks the fact-local level; got: {rejections:?}"
    );
}

/// AC3: an all-channel measure grouped by the generic brand level — RULE 9 must stay silent (FR5).
#[test]
fn ac3_all_channel_measure_unaffected() {
    let catalog = brand_catalog();
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "tpcds.total_quantity_sold".into(),
            aggregation: None,
        }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "product_dimension".into(),
            level: Some("Product Brand Name".into()),
            ..Default::default()
        }],
        filters: vec![],
    };
    let rejections = validate(&mqo, &catalog);
    let r9: Vec<_> = rejections
        .iter()
        .filter(|r| matches!(&r.reason, RejectReason::NonLocalDimensionPath { .. }))
        .collect();
    assert!(
        r9.is_empty(),
        "AC3: RULE 9 must stay silent for an all-channel measure (FR5); got: {rejections:?}"
    );
}

/// AC4: a store measure grouped by a level with NO fact-local sibling — RULE 9 silent (FR4).
#[test]
fn ac4_conformed_only_dimension_unaffected() {
    // Add a "Customer State Name" level that only exists on the generic hierarchy.
    let mut catalog = brand_catalog();
    catalog.dimensions.push(CatalogDimension {
        unique_name: "customer_dimension".into(),
        subject_areas: vec![],
    });
    catalog.hierarchies.push(CatalogHierarchy {
        dimension_unique_name: "customer_dimension".into(),
        hierarchy_unique_name: "customer_dimension".into(),
        levels: vec!["Customer State Name".into()],
        level_meta: vec![],
        fact_local_facts: vec![], // conformed — no fact-local sibling exists
    });

    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "tpcds.store_quantity_sold".into(),
            aggregation: None,
        }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "customer_dimension".into(),
            level: Some("Customer State Name".into()),
            ..Default::default()
        }],
        filters: vec![],
    };
    let rejections = validate(&mqo, &catalog);
    let r9: Vec<_> = rejections
        .iter()
        .filter(|r| matches!(&r.reason, RejectReason::NonLocalDimensionPath { .. }))
        .collect();
    assert!(
        r9.is_empty(),
        "AC4: RULE 9 must stay silent when no fact-local sibling exists (FR4); got: {rejections:?}"
    );
}

/// AC6 (Guardrail 2): measure with no channel_scope at all — RULE 9 silent.
#[test]
fn ac6_no_channel_scope_silent() {
    let mut catalog = brand_catalog();
    catalog.measures.push(CatalogMeasure {
        unique_name: "tpcds.some_other_measure".into(),
        label: Some("Some Other Measure".into()),
        channel_scope: None, // no binding
        ..Default::default()
    });

    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "tpcds.some_other_measure".into(),
            aggregation: None,
        }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "product_dimension".into(),
            level: Some("Product Brand Name".into()),
            ..Default::default()
        }],
        filters: vec![],
    };
    let rejections = validate(&mqo, &catalog);
    let r9: Vec<_> = rejections
        .iter()
        .filter(|r| matches!(&r.reason, RejectReason::NonLocalDimensionPath { .. }))
        .collect();
    assert!(
        r9.is_empty(),
        "AC6: RULE 9 must be silent when channel_scope is absent; got: {rejections:?}"
    );
}

/// Guardrail (updated for PRD-mqo-rule7-conformed-dimension-allowance):
/// RULE 9 and RULE 7 coexistence — conformed dimension + all-channel measure:
/// BOTH rules must be silent (RULE 7 because the dimension is conformed, RULE 9 because
/// the measure is all-channel / not fact-local per FR5).
#[test]
fn r9_coexists_with_r7_all_channel_measure_both_could_be_wrong() {
    // All-channel measure grouped by generic conformed brand dimension.
    // - RULE 7: dimension is conformed (empty fact_local_facts) → ALLOW (R1/R2 of
    //   PRD-mqo-rule7-conformed-dimension-allowance); must be SILENT.
    // - RULE 9: measure is all-channel (not fact-local) → must be SILENT (FR5).
    let catalog = brand_catalog();
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "tpcds.total_quantity_sold".into(),
            aggregation: None,
        }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "product_dimension".into(),
            level: Some("Product Brand Name".into()),
            ..Default::default()
        }],
        filters: vec![],
    };
    let rejections = validate(&mqo, &catalog);
    let r7_count = rejections
        .iter()
        .filter(|r| matches!(&r.reason, RejectReason::ChannelScopeMismatch { .. }))
        .count();
    let r9_count = rejections
        .iter()
        .filter(|r| matches!(&r.reason, RejectReason::NonLocalDimensionPath { .. }))
        .count();
    // RULE 7 must be SILENT: dimension is conformed → canonical all-channel measure allowed.
    assert_eq!(
        r7_count, 0,
        "Guardrail: RULE 7 must be silent for all-channel measure + conformed dimension (PRD-mqo-rule7-conformed-dimension-allowance); got: {rejections:?}"
    );
    // RULE 9 must stay silent (FR5: all-channel measure → not fact-local).
    assert_eq!(
        r9_count, 0,
        "Guardrail: RULE 9 must stay silent for all-channel measure (FR5); got: {rejections:?}"
    );
}

/// Guardrail: RULE 7 still fires when the all-channel measure is paired with a
/// channel-specific dimension (strict subset of the measure's channel scope).
#[test]
fn r7_fires_when_channel_specific_dimension_present() {
    // Extend brand_catalog with a store-only dimension.
    let mut catalog = brand_catalog();
    catalog.dimensions.push(CatalogDimension {
        unique_name: "store_dimension".into(),
        subject_areas: vec![],
    });
    catalog.hierarchies.push(CatalogHierarchy {
        dimension_unique_name: "store_dimension".into(),
        hierarchy_unique_name: "store_dimension".into(),
        levels: vec!["Store City".into()],
        level_meta: vec![],
        fact_local_facts: vec!["store_sales".into()],  // strict subset of {store,catalog,web}
    });

    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "tpcds.total_quantity_sold".into(),
            aggregation: None,
        }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "store_dimension".into(),
            level: Some("Store City".into()),
            ..Default::default()
        }],
        filters: vec![],
    };
    let rejections = validate(&mqo, &catalog);
    let r7_count = rejections
        .iter()
        .filter(|r| matches!(&r.reason, RejectReason::ChannelScopeMismatch { .. }))
        .count();
    assert_eq!(
        r7_count, 1,
        "RULE 7 must still fire when channel-specific dimension conflicts with all-channel measure; got: {rejections:?}"
    );
}
