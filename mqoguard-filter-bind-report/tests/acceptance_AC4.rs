//! AC4 (SHOULD): Given an unbindable member where the catalog defines the
//! level's key shape, the dropped entry includes the expected member-key shape
//! as a suggestion.

use mqoguard_filter_bind_report::{
    BoundMqo, CompiledQuery, DropReason, HierarchyCatalog, HierarchyMeta, LevelMeta, MemberFilter,
    MqoFilter,
};

fn catalog_with_year_shape() -> HierarchyCatalog {
    HierarchyCatalog {
        hierarchies: vec![HierarchyMeta {
            name: "sold_date_dimensions".to_owned(),
            levels: vec![LevelMeta {
                name: "Sold Calendar Year".to_owned(),
                expected_key_shape: Some(
                    "four-digit integer year string, e.g. '2001'".to_owned(),
                ),
            }],
        }],
    }
}

/// Dropped filter with catalog present — suggestion is populated.
#[test]
fn ac4_suggestion_when_catalog_has_level() {
    let mqo = BoundMqo {
        filters: vec![MqoFilter::Member(MemberFilter {
            filter_id: "year-filter".to_owned(),
            hierarchy: "sold_date_dimensions".to_owned(),
            level: Some("Sold Calendar Year".to_owned()),
            members: vec!["year_2001".to_owned()], // wrong key format
        })],
        catalog: Some(catalog_with_year_shape()),
    };
    let compiled = CompiledQuery {
        sql: "SELECT SUM(ss_net_profit) FROM tpcds_benchmark_model".to_owned(),
        bound_filter_ids: None,
    };

    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);

    assert_eq!(report.dropped.len(), 1);
    let dropped = &report.dropped[0];
    assert_eq!(dropped.reason, DropReason::UnbindableMember);

    assert!(
        dropped.suggestion.is_some(),
        "suggestion should be present when catalog defines key shape"
    );
    let suggestion = dropped.suggestion.as_deref().unwrap_or("");
    assert!(
        suggestion.contains("four-digit integer year string"),
        "suggestion should include the expected key shape; got: {suggestion}"
    );
}

/// Dropped filter with catalog present but unknown hierarchy → `UnknownHierarchy`,
/// no suggestion.
#[test]
fn ac4_unknown_hierarchy_no_suggestion() {
    let mqo = BoundMqo {
        filters: vec![MqoFilter::Member(MemberFilter {
            filter_id: "mystery-filter".to_owned(),
            hierarchy: "nonexistent_dim".to_owned(),
            level: None,
            members: vec!["foo".to_owned()],
        })],
        catalog: Some(catalog_with_year_shape()),
    };
    let compiled = CompiledQuery {
        sql: "SELECT 1".to_owned(),
        bound_filter_ids: None,
    };

    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);
    assert_eq!(report.dropped.len(), 1);
    let dropped = &report.dropped[0];
    assert_eq!(dropped.reason, DropReason::UnknownHierarchy);
    assert!(dropped.suggestion.is_none());
}

/// Dropped filter with catalog but unknown level → `UnknownLevel`.
#[test]
fn ac4_unknown_level_no_suggestion() {
    let mqo = BoundMqo {
        filters: vec![MqoFilter::Member(MemberFilter {
            filter_id: "level-filter".to_owned(),
            hierarchy: "sold_date_dimensions".to_owned(),
            level: Some("Nonexistent Level".to_owned()),
            members: vec!["2001".to_owned()],
        })],
        catalog: Some(catalog_with_year_shape()),
    };
    let compiled = CompiledQuery {
        sql: "SELECT 1".to_owned(),
        bound_filter_ids: None,
    };

    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);
    assert_eq!(report.dropped.len(), 1);
    assert_eq!(report.dropped[0].reason, DropReason::UnknownLevel);
}

/// Applied filter with catalog present — no suggestion needed.
#[test]
fn ac4_applied_filter_no_suggestion() {
    let mqo = BoundMqo {
        filters: vec![MqoFilter::Member(MemberFilter {
            filter_id: "year-filter".to_owned(),
            hierarchy: "sold_date_dimensions".to_owned(),
            level: Some("Sold Calendar Year".to_owned()),
            members: vec!["2001".to_owned()],
        })],
        catalog: Some(catalog_with_year_shape()),
    };
    // SQL does contain "2001" — applied.
    let compiled = CompiledQuery {
        sql: "SELECT SUM(ss_net_profit) FROM t WHERE sold_year = '2001'".to_owned(),
        bound_filter_ids: None,
    };

    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);
    assert_eq!(report.applied.len(), 1);
    assert!(report.dropped.is_empty());
}
