//! AC8 (PRD mqo-calc-preference-grounding): packaged-calc preference grounding.
//!
//! - FR-1: `inspect_calcs` flags packaged calc measures `is_calc:true` with a
//!   non-empty `triggers` list (AC-1).
//! - FR-2/FR-3: the validator rejects an MQO that hand-derives a packaged calc
//!   pre-execution, naming the calc via `suggested_calc` (AC-2).
//! - FR-5: no false rejection when there is no packaged-calc equivalent, and on
//!   the other failure modes' canonical (non-re-deriving) MQO shapes (AC-3/AC-5).

use mqo_param_validator::{
    inspect_calcs, is_packaged_calc, validate, BoundMqoInput, CatalogDimension, CatalogMeasure,
    CatalogSnapshot, MqoDimensionRef, MqoFilterRef, MqoMeasureRef, RejectReason,
};

/// A TPC-DS-shaped catalog with packaged Increase/Growth calcs alongside their
/// base series measures.
fn tpcds_calc_catalog() -> CatalogSnapshot {
    let measure = |name: &str| CatalogMeasure {
        unique_name: name.to_string(),
        subject_area: None,
        label: Some(name.to_string()),
        is_calc: None,
            ..Default::default()    };
    CatalogSnapshot {
        measures: vec![
            measure("Total Store Sales"),
            measure("Store Ext Sales Price"),
            measure("Store Sales Increase"),
            measure("Total Web Sales"),
            measure("Web Sales"),
            measure("Web Sales Increase"),
            measure("Web and Catalog Sales Price Growth"),
            measure("Total Net Profit"),
        ],
        dimensions: vec![
            CatalogDimension {
                unique_name: "Sold Calendar Quarter".to_string(),
                subject_areas: vec![],
            },
            CatalogDimension {
                unique_name: "Sold Calendar Month".to_string(),
                subject_areas: vec![],
            },
            CatalogDimension {
                unique_name: "Product Class Name".to_string(),
                subject_areas: vec![],
            },
        ],
        ..Default::default()
    }
}

fn rederivation_rejections(result: &[mqo_param_validator::ParamRejection]) -> Vec<&mqo_param_validator::ParamRejection> {
    result
        .iter()
        .filter(|r| r.reason == RejectReason::ManualCalcRederivation)
        .collect()
}

// --- AC-1: is_calc + triggers surfacing ---------------------------------

#[test]
fn ac1_packaged_calcs_flagged_with_triggers() {
    let catalog = tpcds_calc_catalog();
    let surfaced = inspect_calcs(&catalog);

    // Store Sales Increase must be is_calc:true with a non-empty trigger list.
    let ssi = surfaced
        .iter()
        .find(|s| s.label == "Store Sales Increase")
        .expect("Store Sales Increase surfaced");
    assert!(ssi.is_calc, "Store Sales Increase must be is_calc:true");
    assert!(!ssi.triggers.is_empty(), "triggers must be non-empty");
    assert!(
        ssi.triggers.iter().any(|t| t.contains("store sales increase")),
        "triggers should include the full name: {:?}",
        ssi.triggers
    );
    assert!(
        ssi.triggers.iter().any(|t| t == "increase" || t.contains("vs prior period")),
        "triggers should include calc-kind synonyms: {:?}",
        ssi.triggers
    );

    // A plain base measure must NOT be a calc.
    let total = surfaced
        .iter()
        .find(|s| s.label == "Total Store Sales")
        .unwrap();
    assert!(!total.is_calc, "Total Store Sales is not a calc");
    assert!(total.triggers.is_empty());

    // Growth calc detected too.
    let growth = surfaced
        .iter()
        .find(|s| s.label == "Web and Catalog Sales Price Growth")
        .unwrap();
    assert!(growth.is_calc);
}

#[test]
fn ac1_explicit_is_calc_flag_honored() {
    let m = CatalogMeasure {
        unique_name: "Custom Metric".to_string(),
        subject_area: None,
        label: None,
        is_calc: Some(true),
        ..Default::default()
    };
    assert!(is_packaged_calc(&m), "explicit is_calc:true must win");
}

// --- AC-2: fm5-002 shape rejects and names the calc ---------------------

#[test]
fn ac2_fm5_002_store_sales_increase_rederivation_rejected() {
    // fm5-002: "store sales trending vs prior period per quarter". Hand-rolled
    // as Store Ext Sales Price / lagged Store Ext Sales Price by Sold Calendar
    // Quarter — re-deriving the packaged Store Sales Increase.
    let mqo = BoundMqoInput {
        measures: vec![
            MqoMeasureRef { unique_name: "Store Ext Sales Price".to_string(), ..Default::default() },
            MqoMeasureRef { unique_name: "Prior Store Ext Sales Price".to_string(), ..Default::default() },
        ],
        dimensions: vec![MqoDimensionRef {
            unique_name: "Sold Calendar Quarter".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &tpcds_calc_catalog());
    let rerej = rederivation_rejections(&result);
    assert_eq!(rerej.len(), 1, "expected exactly one rederivation rejection: {result:?}");
    assert_eq!(
        rerej[0].suggested_calc.as_deref(),
        Some("Store Sales Increase"),
        "must name the packaged calc"
    );
}

#[test]
fn ac2_total_store_sales_plus_lagged_by_date_rejected() {
    // PRD AC-2 literal shape: Total Store Sales (+ lagged twin) + a date level.
    let mqo = BoundMqoInput {
        measures: vec![
            MqoMeasureRef { unique_name: "Total Store Sales".to_string(), ..Default::default() },
            MqoMeasureRef { unique_name: "Total Store Sales".to_string(), ..Default::default() },
        ],
        dimensions: vec![MqoDimensionRef {
            unique_name: "Sold Calendar Quarter".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &tpcds_calc_catalog());
    let rerej = rederivation_rejections(&result);
    assert_eq!(rerej.len(), 1, "duplicate base + date → rejection: {result:?}");
    assert_eq!(rerej[0].suggested_calc.as_deref(), Some("Store Sales Increase"));
}

#[test]
fn ac2_fm5_003_web_sales_increase_rederivation_rejected() {
    // fm5-003: "Web sales prior-period change for each month of 2001."
    let mqo = BoundMqoInput {
        measures: vec![
            MqoMeasureRef { unique_name: "Web Sales".to_string(), ..Default::default() },
            MqoMeasureRef { unique_name: "Prior Period Web Sales".to_string(), ..Default::default() },
        ],
        dimensions: vec![MqoDimensionRef {
            unique_name: "Sold Calendar Month".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &tpcds_calc_catalog());
    let rerej = rederivation_rejections(&result);
    assert_eq!(rerej.len(), 1, "fm5-003 shape → rejection: {result:?}");
    assert_eq!(rerej[0].suggested_calc.as_deref(), Some("Web Sales Increase"));
}

// --- AC-3 / AC-5: no false positives ------------------------------------

#[test]
fn ac3_no_packaged_calc_equivalent_not_rejected() {
    // Total Net Profit has no packaged Increase/Growth calc → never rejected,
    // even with a lagged twin + date axis.
    let mqo = BoundMqoInput {
        measures: vec![
            MqoMeasureRef { unique_name: "Total Net Profit".to_string(), ..Default::default() },
            MqoMeasureRef { unique_name: "Prior Total Net Profit".to_string(), ..Default::default() },
        ],
        dimensions: vec![MqoDimensionRef {
            unique_name: "Sold Calendar Month".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    // Note: "Prior Total Net Profit" is unmapped, but no rederivation rejection.
    let result = validate(&mqo, &tpcds_calc_catalog());
    assert!(
        rederivation_rejections(&result).is_empty(),
        "no calc equivalent → no rederivation rejection: {result:?}"
    );
}

#[test]
fn ac5_plain_base_measure_by_date_not_rejected() {
    // fm2-006 canonical: Total Store Sales by Sold Calendar Quarter. A single
    // base measure with NO lag signal must NOT be rejected even though
    // Store Sales Increase exists in the catalog.
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef { unique_name: "Total Store Sales".to_string(), ..Default::default() }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "Sold Calendar Quarter".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &tpcds_calc_catalog());
    assert!(
        rederivation_rejections(&result).is_empty(),
        "plain base-by-date must not be rejected: {result:?}"
    );
}

#[test]
fn ac5_already_using_calc_not_rejected() {
    // Caller already uses Store Sales Increase → never rejected.
    let mqo = BoundMqoInput {
        measures: vec![MqoMeasureRef { unique_name: "Store Sales Increase".to_string(), ..Default::default() }],
        dimensions: vec![MqoDimensionRef {
            unique_name: "Sold Calendar Quarter".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &tpcds_calc_catalog());
    assert!(rederivation_rejections(&result).is_empty(), "{result:?}");
}

#[test]
fn ac5_no_date_axis_not_rejected() {
    // Duplicate base measure but no date axis → not a period-over-period
    // re-derivation, do not reject.
    let mqo = BoundMqoInput {
        measures: vec![
            MqoMeasureRef { unique_name: "Total Store Sales".to_string(), ..Default::default() },
            MqoMeasureRef { unique_name: "Total Store Sales".to_string(), ..Default::default() },
        ],
        dimensions: vec![MqoDimensionRef {
            unique_name: "Product Class Name".to_string(),
            level: None,
            hierarchy: None,
            role_qualifier: None,
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &tpcds_calc_catalog());
    assert!(rederivation_rejections(&result).is_empty(), "{result:?}");
}

#[test]
fn ac5_other_failure_mode_canonicals_not_rejected() {
    // Canonical (correct) MQOs from the other four failure modes — all single
    // base measures over a date/dim axis. None re-derive a calc.
    let cases: Vec<(Vec<&str>, Vec<&str>, Vec<&str>)> = vec![
        // fm1-003 wrong_date_role
        (vec!["Web Sales"], vec![], vec!["Sold Calendar Quarter"]),
        // fm2-005 wrong_hierarchy_level
        (vec!["Total Store Sales"], vec!["Sold Calendar Month"], vec![]),
        // fm2-013 wrong_hierarchy_level
        (vec!["Total Store Sales"], vec!["Sold Calendar Month"], vec![]),
        // fm4-002 lookalike_measure
        (vec!["Total Store Sales"], vec![], vec!["Sold Calendar Year"]),
    ];
    let catalog = tpcds_calc_catalog();
    for (measures, dims, filters) in cases {
        let mqo = BoundMqoInput {
            measures: measures
                .iter()
                .map(|m| MqoMeasureRef { unique_name: m.to_string(), ..Default::default() })
                .collect(),
            dimensions: dims
                .iter()
                .map(|d| MqoDimensionRef {
                    unique_name: d.to_string(),
                    level: None,
                    hierarchy: None,
                    role_qualifier: None,
                })
                .collect(),
            filters: filters
                .iter()
                .map(|f| MqoFilterRef { unique_name: f.to_string(), level: None, ..Default::default() })
                .collect(),
        };
        let result = validate(&mqo, &catalog);
        assert!(
            rederivation_rejections(&result).is_empty(),
            "canonical MQO {measures:?}/{dims:?}/{filters:?} must not be rejected: {result:?}"
        );
    }
}
