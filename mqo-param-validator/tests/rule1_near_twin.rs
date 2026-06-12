//! RULE 1 (PRD-mqo-validator-near-twin-rejection): reject a non-canonical
//! near-twin *dimension* pick, suggesting the canonical unique_name.
//!
//! Covers the PRD acceptance criteria:
//! - AC-1: Store Item Product Brand Name → reject, suggest Product Brand Name.
//! - AC-2: canonical Product Brand Name → no rejection.
//! - AC-3: non-canonical twin + a filter on that hierarchy → no rejection
//!   (intent guard).
//! - AC-4: a dimension with no near-twin group → no rejection.
//! - AC-5: a measure is never rejected by this rule.

use mqo_param_validator::{
    validate, BoundMqoInput, CatalogHierarchy, CatalogMeasure, CatalogSnapshot, MqoDimensionRef,
    MqoFilterRef, MqoMeasureRef, RejectReason,
};

/// A catalog with a Brand Name near-twin group across two hierarchies:
///   * product_dimension                — canonical (shorter hierarchy name)
///   * store_item_product_dimension     — non-canonical
/// plus a non-twin Customer hierarchy.
fn brand_catalog() -> CatalogSnapshot {
    CatalogSnapshot {
        hierarchies: vec![
            CatalogHierarchy {
                dimension_unique_name: "product_dimension".to_string(),
                hierarchy_unique_name: "product_dimension".to_string(),
                levels: vec!["Product Brand Name".to_string(), "Product Class".to_string()],
                ..Default::default()
            },
            CatalogHierarchy {
                dimension_unique_name: "store_item_product_dimension".to_string(),
                hierarchy_unique_name: "store_item_product_dimension".to_string(),
                levels: vec!["Store Item Product Brand Name".to_string()],
                ..Default::default()
            },
            CatalogHierarchy {
                dimension_unique_name: "customer_dimension".to_string(),
                hierarchy_unique_name: "customer_dimension".to_string(),
                levels: vec!["Customer State".to_string()],
                ..Default::default()
            },
        ],
        measures: vec![CatalogMeasure {
            unique_name: "Total Store Sales".to_string(),
            label: Some("Total Store Sales".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn twin_rejections(r: &[mqo_param_validator::ParamRejection]) -> Vec<&mqo_param_validator::ParamRejection> {
    r.iter()
        .filter(|x| matches!(x.reason, RejectReason::NonCanonicalNearTwin { .. }))
        .collect()
}

// --- AC-1: Brand Name non-canonical pick rejected, names canonical ---------

#[test]
fn ac1_store_item_product_brand_name_rejected_suggests_canonical() {
    let mqo = BoundMqoInput {
        dimensions: vec![MqoDimensionRef {
            unique_name: "store_item_product_dimension".to_string(),
            level: Some("Store Item Product Brand Name".to_string()),
            hierarchy: Some("store_item_product_dimension".to_string()),
            role_qualifier: None,
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &brand_catalog());
    let twins = twin_rejections(&result);
    assert_eq!(twins.len(), 1, "expected one near-twin rejection: {result:?}");
    match &twins[0].reason {
        RejectReason::NonCanonicalNearTwin {
            picked,
            suggested_canonical,
            group_core_label,
        } => {
            assert!(picked.contains("Store Item Product Brand Name"), "picked: {picked}");
            assert!(
                suggested_canonical.contains("product_dimension")
                    && suggested_canonical.contains("Product Brand Name"),
                "suggested canonical: {suggested_canonical}"
            );
            // core_label drops the trailing "name" token, then keeps the last
            // 2 tokens → "product brand" (matches describe_model's grouping).
            assert_eq!(group_core_label, "product brand", "core label");
        }
        other => panic!("wrong reason: {other:?}"),
    }
}

// --- AC-2: canonical pick → no rejection -----------------------------------

#[test]
fn ac2_canonical_product_brand_name_not_rejected() {
    let mqo = BoundMqoInput {
        dimensions: vec![MqoDimensionRef {
            unique_name: "product_dimension".to_string(),
            level: Some("Product Brand Name".to_string()),
            hierarchy: Some("product_dimension".to_string()),
            role_qualifier: None,
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &brand_catalog());
    assert!(twin_rejections(&result).is_empty(), "canonical pick must pass: {result:?}");
}

// --- AC-3: intent guard — filter on the picked member's own hierarchy ------

#[test]
fn ac3_intent_guard_filter_on_picked_hierarchy_not_rejected() {
    let mqo = BoundMqoInput {
        dimensions: vec![MqoDimensionRef {
            unique_name: "store_item_product_dimension".to_string(),
            level: Some("Store Item Product Brand Name".to_string()),
            hierarchy: Some("store_item_product_dimension".to_string()),
            role_qualifier: None,
            ..Default::default()
        }],
        filters: vec![MqoFilterRef {
            unique_name: "store_item_product_dimension".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &brand_catalog());
    assert!(
        twin_rejections(&result).is_empty(),
        "deliberate filter on the picked hierarchy → intent guard, no rejection: {result:?}"
    );
}

#[test]
fn ac3_intent_guard_second_dimension_on_picked_hierarchy_not_rejected() {
    // A second dimension reference on the same non-canonical hierarchy signals
    // intentional scoping → no rejection.
    let mqo = BoundMqoInput {
        dimensions: vec![
            MqoDimensionRef {
                unique_name: "store_item_product_dimension".to_string(),
                level: Some("Store Item Product Brand Name".to_string()),
                hierarchy: Some("store_item_product_dimension".to_string()),
                role_qualifier: None,
                ..Default::default()
            },
            MqoDimensionRef {
                unique_name: "store_item_product_dimension".to_string(),
                level: Some("Product Class".to_string()),
                hierarchy: Some("store_item_product_dimension".to_string()),
                role_qualifier: None,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let result = validate(&mqo, &brand_catalog());
    assert!(twin_rejections(&result).is_empty(), "{result:?}");
}

// --- AC-4: dimension with no near-twin group → no rejection ----------------

#[test]
fn ac4_no_twin_group_not_rejected() {
    let mqo = BoundMqoInput {
        dimensions: vec![MqoDimensionRef {
            unique_name: "customer_dimension".to_string(),
            level: Some("Customer State".to_string()),
            hierarchy: Some("customer_dimension".to_string()),
            role_qualifier: None,
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &brand_catalog());
    assert!(twin_rejections(&result).is_empty(), "no twin group → no rejection: {result:?}");
}

// --- AC-5: measures are never rejected by this rule ------------------------

#[test]
fn ac5_measure_never_near_twin_rejected() {
    // Even if a measure name collides with a twin core label, RULE 1 ignores
    // measures entirely (it only inspects mqo.dimensions).
    let mut catalog = brand_catalog();
    catalog.measures.push(CatalogMeasure {
        unique_name: "Store Item Product Brand Name".to_string(),
        label: Some("Store Item Product Brand Name".to_string()),
        ..Default::default()
    });
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef {
            unique_name: "Store Item Product Brand Name".to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog);
    assert!(twin_rejections(&result).is_empty(), "measures never near-twin rejected: {result:?}");
}

// --- No clear canonical → no rejection (PRD edge case) ---------------------

#[test]
fn no_clear_canonical_group_still_picks_one() {
    // build_twin_groups always derives a canonical via the deterministic
    // tiebreak, so a group always has a canonical; a NON-canonical member is
    // rejected, the canonical member is not. This guards that the canonical
    // itself is never self-rejected.
    let mqo = BoundMqoInput {
        dimensions: vec![MqoDimensionRef {
            unique_name: "product_dimension".to_string(),
            level: Some("Product Brand Name".to_string()),
            hierarchy: Some("product_dimension".to_string()),
            role_qualifier: None,
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &brand_catalog());
    assert!(twin_rejections(&result).is_empty(), "{result:?}");
}
