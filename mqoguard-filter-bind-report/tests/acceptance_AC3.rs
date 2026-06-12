//! AC3: Given the canonical "2001" year filter that the engine drops (the
//! corpus case from the 2026-06-09 mcp-tuner run), `report_filters` places it in
//! `dropped` with reason `UnbindableMember`.

use mqoguard_filter_bind_report::{
    BoundMqo, CompiledQuery, DropReason, MemberFilter, MqoFilter,
};

/// The exact scenario observed across multiple tasks in the 2026-06-09 k=4 run:
/// agent sent `{"hierarchy":"sold_date_dimensions","members":["2001"]}` and the
/// server emitted SQL with no WHERE clause, returning all-years totals.
#[test]
fn ac3_canonical_year_2001_dropped_unbindable() {
    let mqo = BoundMqo {
        filters: vec![MqoFilter::Member(MemberFilter {
            filter_id: "year-filter".to_owned(),
            hierarchy: "sold_date_dimensions".to_owned(),
            level: None,
            members: vec!["2001".to_owned()],
        })],
        catalog: None,
    };
    // SQL has no WHERE clause referencing 2001 — the server dropped the filter.
    let compiled = CompiledQuery {
        sql: "SELECT SUM(ss_net_profit) FROM postgres.tpcds.tpcds_benchmark_model".to_owned(),
        bound_filter_ids: None,
    };

    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);

    assert_eq!(report.input_filter_count, 1);
    assert!(report.applied.is_empty(), "year filter should NOT be in applied");
    assert_eq!(report.dropped.len(), 1);

    let dropped = &report.dropped[0];
    assert_eq!(dropped.filter.filter_id(), "year-filter");
    assert_eq!(
        dropped.reason,
        DropReason::UnbindableMember,
        "expected UnbindableMember; got {:?}",
        dropped.reason
    );
}

/// Same scenario but with the compiler's exact binding record confirming the drop.
#[test]
fn ac3_canonical_year_2001_dropped_exact() {
    let mqo = BoundMqo {
        filters: vec![MqoFilter::Member(MemberFilter {
            filter_id: "year-filter".to_owned(),
            hierarchy: "sold_date_dimensions".to_owned(),
            level: None,
            members: vec!["2001".to_owned()],
        })],
        catalog: None,
    };
    let compiled = CompiledQuery {
        sql: "SELECT SUM(ss_net_profit) FROM postgres.tpcds.tpcds_benchmark_model".to_owned(),
        // Compiler bound zero filters.
        bound_filter_ids: Some(vec![]),
    };

    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);

    assert!(report.applied.is_empty());
    assert_eq!(report.dropped.len(), 1);
    assert_eq!(report.dropped[0].reason, DropReason::UnbindableMember);
}
