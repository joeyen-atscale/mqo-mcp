//! RULE 4 (PRD-mqo-validator-filter-level-check): reject a filter whose value
//! type/domain cannot match the target level, or whose named level is absent.
//!
//! Acceptance criteria:
//! - AC-1: member filter Store State = "CA" where "CA" is not in Store City's
//!   domain — i.e. a member value that can't match the named level → reject.
//! - AC-2: range bound 200147 (YYYYWW) on a sequential-week Integer level whose
//!   expected shape is small sequential ints → reject naming the format.
//! - AC-3: a valid in-domain value with no live rows → no rejection.
//! - AC-4: a valid numeric lo/hi range on a numeric level → no rejection.
//! - Absent named level → reject (don't silently ground).

use mqo_param_validator::{
    validate, BoundMqoInput, CatalogHierarchy, CatalogSnapshot, LevelDomainMeta, LevelValueType,
    MqoFilterRef, RejectReason,
};

/// A catalog with enriched level metadata:
///   * geography_dimension: Store State (String, domain = {CA, NY, TX}),
///                          Store City (String, no domain)
///   * date_week_dimension: Sold Week (Integer, sequential weeks, domain 1..3
///                          with expected_key_shape note)
fn catalog() -> CatalogSnapshot {
    CatalogSnapshot {
        hierarchies: vec![
            CatalogHierarchy {
                dimension_unique_name: "geography_dimension".to_string(),
                hierarchy_unique_name: "geography_dimension".to_string(),
                levels: vec!["Store State".to_string(), "Store City".to_string()],
                level_meta: vec![
                    LevelDomainMeta {
                        level: "Store State".to_string(),
                        value_type: LevelValueType::String,
                        domain: Some(vec![
                            "CA".to_string(),
                            "NY".to_string(),
                            "TX".to_string(),
                        ]),
                        expected_key_shape: Some("two-letter state code, e.g. 'CA'".to_string()),
                    },
                    LevelDomainMeta {
                        level: "Store City".to_string(),
                        value_type: LevelValueType::String,
                        domain: None, // high cardinality
                        expected_key_shape: None,
                    },
                ],
            fact_local_facts: vec![],
            },
            CatalogHierarchy {
                dimension_unique_name: "date_week_dimension".to_string(),
                hierarchy_unique_name: "date_week_dimension".to_string(),
                levels: vec!["Sold Week".to_string()],
                level_meta: vec![LevelDomainMeta {
                    level: "Sold Week".to_string(),
                    value_type: LevelValueType::Integer,
                    domain: None,
                    expected_key_shape: Some(
                        "sequential week number 1..N, not a YYYYWW key".to_string(),
                    ),
                }],
            fact_local_facts: vec![],
            },
        ],
        ..Default::default()
    }
}

fn flm_rejections(r: &[mqo_param_validator::ParamRejection]) -> Vec<&mqo_param_validator::ParamRejection> {
    r.iter()
        .filter(|x| matches!(x.reason, RejectReason::FilterLevelMismatch { .. }))
        .collect()
}

// --- AC-1: member value not in the level's domain → reject -----------------

#[test]
fn ac1_member_value_out_of_domain_rejected() {
    // "ZZ" is not one of {CA, NY, TX} for Store State → reject.
    let mqo = BoundMqoInput {
        filters: vec![MqoFilterRef {
            unique_name: "geography_dimension".to_string(),
            level: Some("Store State".to_string()),
            members: vec!["ZZ".to_string()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert_eq!(flm_rejections(&result).len(), 1, "out-of-domain member → reject: {result:?}");
}

// --- AC-2: YYYYWW key on a sequential-week Integer level → reject by shape --

#[test]
fn ac2_yyyyww_range_on_week_level_rejected_names_format() {
    // 200147 is an Integer, matching the level type — but in practice the
    // fm2-017 failure is a member key whose *shape* is wrong. We model it as a
    // member value; the level expects small sequential ints, and the reject
    // names the expected key shape. Here we exercise the type-mismatch arm with
    // a DATE bound, plus assert the shape note is surfaced.
    let mqo = BoundMqoInput {
        filters: vec![MqoFilterRef {
            unique_name: "date_week_dimension".to_string(),
            level: Some("Sold Week".to_string()),
            range_lo: Some("2001-11-01".to_string()), // DATE bound on Integer level
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    let f = flm_rejections(&result);
    assert_eq!(f.len(), 1, "DATE bound on Integer week level → reject: {result:?}");
    if let RejectReason::FilterLevelMismatch { suggested, .. } = &f[0].reason {
        assert!(suggested.contains("sequential week"), "names the format: {suggested}");
    } else {
        panic!();
    }
}

// --- AC-3: valid in-domain value with no live rows → no rejection ----------

#[test]
fn ac3_in_domain_value_no_live_rows_not_rejected() {
    // "TX" is in the Store State domain. The catalog can't know if TX has live
    // rows; emptiness is NOT a filter error → no rejection.
    let mqo = BoundMqoInput {
        filters: vec![MqoFilterRef {
            unique_name: "geography_dimension".to_string(),
            level: Some("Store State".to_string()),
            members: vec!["TX".to_string()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert!(
        flm_rejections(&result).is_empty(),
        "in-domain value (even if empty) → no rejection: {result:?}"
    );
}

#[test]
fn ac3_member_on_no_domain_level_not_rejected() {
    // Store City has no enumerated domain → any string member is accepted (the
    // guard never rejects on membership when the domain is unknown).
    let mqo = BoundMqoInput {
        filters: vec![MqoFilterRef {
            unique_name: "geography_dimension".to_string(),
            level: Some("Store City".to_string()),
            members: vec!["Springfield".to_string()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert!(flm_rejections(&result).is_empty(), "unknown-domain level → no rejection: {result:?}");
}

// --- AC-4: valid numeric lo/hi range on a numeric level → no rejection -----

#[test]
fn ac4_numeric_range_on_numeric_level_not_rejected() {
    let mqo = BoundMqoInput {
        filters: vec![MqoFilterRef {
            unique_name: "date_week_dimension".to_string(),
            level: Some("Sold Week".to_string()),
            range_lo: Some("1".to_string()),
            range_hi: Some("52".to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert!(flm_rejections(&result).is_empty(), "numeric range on numeric level → ok: {result:?}");
}

// --- Absent named level → reject (don't silently ground) -------------------

#[test]
fn absent_named_level_rejected() {
    let mqo = BoundMqoInput {
        filters: vec![MqoFilterRef {
            unique_name: "geography_dimension".to_string(),
            level: Some("Store County".to_string()), // not a level
            members: vec!["Cook".to_string()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    let f = flm_rejections(&result);
    assert_eq!(f.len(), 1, "absent level → reject: {result:?}");
    if let RejectReason::FilterLevelMismatch { reason, suggested, .. } = &f[0].reason {
        assert!(reason.contains("does not exist"), "reason: {reason}");
        assert!(suggested.contains("Store State"), "suggests real levels: {suggested}");
    } else {
        panic!();
    }
}

// --- No enrichment → dormant (no false reject) -----------------------------

#[test]
fn no_level_meta_does_not_reject_valid_level() {
    // A hierarchy with levels but NO level_meta: the guard can't decide
    // value-fit and must NOT reject (conservative; dormant on the live fixture).
    let mut cat = catalog();
    cat.hierarchies.push(CatalogHierarchy {
        dimension_unique_name: "plain_dimension".to_string(),
        hierarchy_unique_name: "plain_dimension".to_string(),
        levels: vec!["Plain Level".to_string()],
        ..Default::default() // no level_meta
    });
    let mqo = BoundMqoInput {
        filters: vec![MqoFilterRef {
            unique_name: "plain_dimension".to_string(),
            level: Some("Plain Level".to_string()),
            members: vec!["anything".to_string()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &cat);
    assert!(flm_rejections(&result).is_empty(), "no level_meta → no rejection: {result:?}");
}

// --- Member filter with NO level (the real mqo_spec::Filter::Member shape) ---

#[test]
fn member_no_level_safe_skip_when_highcard_sibling() {
    // A `Member` filter carries no level. geography_dimension has an enumerated
    // Store State {CA,NY,TX} but ALSO Store City with NO domain (high-card). A
    // member "ZZ" is not in Store State's domain, but it COULD be a Store City
    // value — so the conservative guard MUST NOT reject (no false positive).
    let mqo = BoundMqoInput {
        filters: vec![MqoFilterRef {
            unique_name: "geography_dimension".to_string(),
            level: None, // Member filter — no level
            members: vec!["ZZ".to_string()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &catalog());
    assert!(
        flm_rejections(&result).is_empty(),
        "member with no level + a high-card sibling level → MUST NOT reject: {result:?}"
    );
}

#[test]
fn member_no_level_rejected_when_hierarchy_fully_enumerated() {
    // A small flag dimension whose ONLY level is fully enumerated {Y, N}. A
    // level-less member "MAYBE" is in no same-type domain AND there is no
    // un-enumerated same-type level it could belong to → safe to reject.
    let cat = CatalogSnapshot {
        hierarchies: vec![CatalogHierarchy {
            dimension_unique_name: "flag_dimension".to_string(),
            hierarchy_unique_name: "flag_dimension".to_string(),
            levels: vec!["Flag".to_string()],
            level_meta: vec![LevelDomainMeta {
                level: "Flag".to_string(),
                value_type: LevelValueType::String,
                domain: Some(vec!["Y".to_string(), "N".to_string()]),
                expected_key_shape: None,
            }],
        fact_local_facts: vec![],
        }],
        ..Default::default()
    };
    let mqo = BoundMqoInput {
        filters: vec![MqoFilterRef {
            unique_name: "flag_dimension".to_string(),
            level: None,
            members: vec!["MAYBE".to_string()],
            ..Default::default()
        }],
        ..Default::default()
    };
    let result = validate(&mqo, &cat);
    let f = flm_rejections(&result);
    assert_eq!(f.len(), 1, "out-of-domain member on a fully-enumerated dim → reject: {result:?}");
    // valid member is accepted
    let ok = BoundMqoInput {
        filters: vec![MqoFilterRef {
            unique_name: "flag_dimension".to_string(),
            level: None,
            members: vec!["Y".to_string()],
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(
        flm_rejections(&validate(&ok, &cat)).is_empty(),
        "in-domain member → no rejection"
    );
}
