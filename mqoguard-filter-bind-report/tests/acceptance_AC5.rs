//! AC5: SQL string-matching (fallback) marks entries as heuristic confidence;
//! exact compiler binding record marks entries as exact.

use mqoguard_filter_bind_report::{
    BoundMqo, CompiledQuery, DetectionConfidence, DetectionMode, MemberFilter, MqoFilter,
};

fn make_member(id: &str, hierarchy: &str, members: &[&str]) -> MqoFilter {
    MqoFilter::Member(MemberFilter {
        filter_id: id.to_owned(),
        hierarchy: hierarchy.to_owned(),
        level: None,
        members: members.iter().map(|s| (*s).to_owned()).collect(),
    })
}

/// When no `bound_filter_ids` present: heuristic mode, heuristic confidence.
#[test]
fn ac5_heuristic_when_no_binding_record() {
    let mqo = BoundMqo {
        filters: vec![
            make_member("f1", "store_dim", &["store_42"]),
            make_member("f2", "sold_date_dimensions", &["2001"]),
        ],
        catalog: None,
    };
    let compiled = CompiledQuery {
        sql: "SELECT SUM(x) FROM t WHERE store_id = 'store_42'".to_owned(),
        bound_filter_ids: None, // no record → heuristic
    };

    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);
    assert_eq!(report.detection_mode, DetectionMode::HeuristicSqlSearch);

    for af in &report.applied {
        assert_eq!(
            af.confidence,
            DetectionConfidence::Heuristic,
            "applied entries should be heuristic when no binding record"
        );
    }
    for df in &report.dropped {
        assert_eq!(
            df.confidence,
            DetectionConfidence::Heuristic,
            "dropped entries should be heuristic when no binding record"
        );
    }
}

/// When `bound_filter_ids` present: exact mode, exact confidence.
#[test]
fn ac5_exact_when_binding_record_present() {
    let mqo = BoundMqo {
        filters: vec![
            make_member("f1", "store_dim", &["store_42"]),
            make_member("f2", "sold_date_dimensions", &["2001"]),
        ],
        catalog: None,
    };
    let compiled = CompiledQuery {
        sql: "SELECT SUM(x) FROM t WHERE store_id = 'store_42'".to_owned(),
        bound_filter_ids: Some(vec!["f1".to_owned()]), // f2 not bound
    };

    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);
    assert_eq!(report.detection_mode, DetectionMode::ExactBindingRecord);

    for af in &report.applied {
        assert_eq!(
            af.confidence,
            DetectionConfidence::Exact,
            "applied entries should be exact when binding record present"
        );
    }
    for df in &report.dropped {
        assert_eq!(
            df.confidence,
            DetectionConfidence::Exact,
            "dropped entries should be exact when binding record present"
        );
    }

    // Verify the right filter was applied.
    assert_eq!(report.applied.len(), 1);
    assert_eq!(report.applied[0].filter.filter_id(), "f1");
    assert_eq!(report.dropped.len(), 1);
    assert_eq!(report.dropped[0].filter.filter_id(), "f2");
}

/// Empty `bound_filter_ids` means compiler bound nothing → all dropped, exact.
#[test]
fn ac5_empty_bound_ids_all_dropped_exact() {
    let mqo = BoundMqo {
        filters: vec![make_member("f1", "sold_date_dimensions", &["2001"])],
        catalog: None,
    };
    let compiled = CompiledQuery {
        sql: "SELECT SUM(x) FROM t".to_owned(),
        bound_filter_ids: Some(vec![]), // compiler bound nothing
    };

    let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);
    assert_eq!(report.detection_mode, DetectionMode::ExactBindingRecord);
    assert!(report.applied.is_empty());
    assert_eq!(report.dropped.len(), 1);
    assert_eq!(report.dropped[0].confidence, DetectionConfidence::Exact);
}
