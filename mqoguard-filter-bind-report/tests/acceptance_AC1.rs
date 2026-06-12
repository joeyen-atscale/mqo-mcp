//! AC1: Given an MQO with two filters where one binds and one does not,
//! `report_filters` returns `applied=[bound]` and `dropped=[unbound]` each typed.

use mqoguard_filter_bind_report::{
    BoundMqo, CompiledQuery, DetectionConfidence, DropReason, MemberFilter, MqoFilter,
};

fn make_member(id: &str, hierarchy: &str, members: &[&str]) -> MqoFilter {
    MqoFilter::Member(MemberFilter {
        filter_id: id.to_owned(),
        hierarchy: hierarchy.to_owned(),
        level: None,
        members: members.iter().map(|s| (*s).to_owned()).collect(),
    })
}

/// Heuristic path: one filter present in SQL, one absent.
#[test]
fn ac1_one_applied_one_dropped_heuristic() {
    let mqo = BoundMqo {
        filters: vec![
            make_member("f-store", "store_dim", &["store_42"]),
            make_member("f-year", "sold_date_dimensions", &["2001"]),
        ],
        catalog: None,
    };
    // SQL mentions store_42 but NOT 2001 — year filter was dropped.
    let compiled = CompiledQuery {
        sql: "SELECT SUM(ss_net_profit) FROM tpcds WHERE store_id = 'store_42'".to_owned(),
        bound_filter_ids: None,
    };
    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);

    assert_eq!(report.input_filter_count, 2);
    assert_eq!(report.applied.len(), 1);
    assert_eq!(report.dropped.len(), 1);

    let applied = &report.applied[0];
    assert_eq!(applied.filter.filter_id(), "f-store");
    assert_eq!(applied.confidence, DetectionConfidence::Heuristic);

    let dropped = &report.dropped[0];
    assert_eq!(dropped.filter.filter_id(), "f-year");
    assert_eq!(dropped.reason, DropReason::UnbindableMember);
    assert_eq!(dropped.confidence, DetectionConfidence::Heuristic);
}

/// Exact path: compiler `bound_filter_ids` tells us precisely which bound.
#[test]
fn ac1_one_applied_one_dropped_exact() {
    let mqo = BoundMqo {
        filters: vec![
            make_member("f-store", "store_dim", &["store_42"]),
            make_member("f-year", "sold_date_dimensions", &["2001"]),
        ],
        catalog: None,
    };
    let compiled = CompiledQuery {
        sql: "SELECT SUM(ss_net_profit) FROM tpcds WHERE store_id = 'store_42'".to_owned(),
        bound_filter_ids: Some(vec!["f-store".to_owned()]),
    };
    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);

    assert_eq!(report.input_filter_count, 2);
    assert_eq!(report.applied.len(), 1);
    assert_eq!(report.dropped.len(), 1);

    let applied = &report.applied[0];
    assert_eq!(applied.filter.filter_id(), "f-store");
    assert_eq!(applied.confidence, DetectionConfidence::Exact);

    let dropped = &report.dropped[0];
    assert_eq!(dropped.filter.filter_id(), "f-year");
    assert_eq!(dropped.reason, DropReason::UnbindableMember);
    assert_eq!(dropped.confidence, DetectionConfidence::Exact);
}
