//! AC6: Given a malformed MQO or compiled query, `report_filters` returns a typed
//! result or fully-dropped report — never panics.

use mqoguard_filter_bind_report::{
    BoundMqo, CompiledQuery, MemberFilter, MqoFilter,
};

fn make_member(id: &str, hierarchy: &str, members: &[&str]) -> MqoFilter {
    MqoFilter::Member(MemberFilter {
        filter_id: id.to_owned(),
        hierarchy: hierarchy.to_owned(),
        level: None,
        members: members.iter().map(|s| (*s).to_owned()).collect(),
    })
}

/// Empty MQO + empty SQL: no panic, empty report.
#[test]
fn ac6_empty_mqo_empty_sql_no_panic() {
    let mqo = BoundMqo {
        filters: vec![],
        catalog: None,
    };
    let compiled = CompiledQuery {
        sql: String::new(),
        bound_filter_ids: None,
    };
    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);
    assert_eq!(report.input_filter_count, 0);
}

/// Filter with empty members list: no panic, treated as dropped (can't bind).
#[test]
fn ac6_empty_members_no_panic() {
    let mqo = BoundMqo {
        filters: vec![MqoFilter::Member(MemberFilter {
            filter_id: "f1".to_owned(),
            hierarchy: "sold_date_dimensions".to_owned(),
            level: None,
            members: vec![], // empty members
        })],
        catalog: None,
    };
    let compiled = CompiledQuery {
        sql: "SELECT SUM(x) FROM t".to_owned(),
        bound_filter_ids: None,
    };
    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);
    assert_eq!(report.input_filter_count, 1);
    // Empty members → can't appear in SQL → treated as dropped.
    assert!(report.applied.is_empty());
    assert_eq!(report.dropped.len(), 1);
}

/// Malformed (non-UTF-8-safe but valid Rust string) SQL: no panic.
#[test]
fn ac6_unusual_sql_no_panic() {
    let mqo = BoundMqo {
        filters: vec![make_member("f1", "dim", &["val"])],
        catalog: None,
    };
    let compiled = CompiledQuery {
        sql: "\x00\x01\x02\x03".to_owned(),
        bound_filter_ids: None,
    };
    let _ = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);
    // Passes if no panic.
}

/// `bound_filter_ids` contains an ID not in the MQO: no panic, the extra ID is
/// simply ignored; only actual MQO filters are classified.
#[test]
fn ac6_extra_bound_id_no_panic() {
    let mqo = BoundMqo {
        filters: vec![make_member("f1", "dim", &["val"])],
        catalog: None,
    };
    let compiled = CompiledQuery {
        sql: "SELECT val FROM t WHERE dim_col = 'val'".to_owned(),
        bound_filter_ids: Some(vec!["f1".to_owned(), "ghost-id".to_owned()]),
    };
    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);
    assert_eq!(report.input_filter_count, 1);
    assert_eq!(report.applied.len(), 1);
    assert!(report.dropped.is_empty());
}

/// Large number of filters: no panic, invariant holds.
#[test]
fn ac6_large_filter_set_no_panic() {
    let filters: Vec<MqoFilter> = (0..1000_usize)
        .map(|i| make_member(&format!("f{i}"), "some_dim", &[&format!("v{i}")]))
        .collect();
    let filter_count = filters.len();
    let mqo = BoundMqo {
        filters,
        catalog: None,
    };
    let compiled = CompiledQuery {
        sql: "SELECT SUM(x) FROM t".to_owned(),
        bound_filter_ids: None,
    };
    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);
    assert_eq!(report.input_filter_count, filter_count);
    assert_eq!(report.applied.len() + report.dropped.len(), filter_count);
}
