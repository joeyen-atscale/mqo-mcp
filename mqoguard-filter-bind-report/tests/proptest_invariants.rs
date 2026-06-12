//! Proptest invariants for the `FilterBindReport` — supplementary to AC2.

use mqoguard_filter_bind_report::{BoundMqo, CompiledQuery, MemberFilter, MqoFilter};
use proptest::prelude::*;

fn arb_filter_id() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{0,15}"
}

fn arb_hierarchy() -> impl Strategy<Value = String> {
    "[a-z_]{3,25}"
}

fn arb_members() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec("[a-z0-9_]{1,12}", 0..=5usize)
}

fn arb_member_filter() -> impl Strategy<Value = MqoFilter> {
    (arb_filter_id(), arb_hierarchy(), arb_members()).prop_map(|(id, hierarchy, members)| {
        MqoFilter::Member(MemberFilter {
            filter_id: id,
            hierarchy,
            level: None,
            members,
        })
    })
}

/// Generate a [`BoundMqo`] with unique filter IDs.
fn arb_bound_mqo() -> impl Strategy<Value = BoundMqo> {
    prop::collection::vec(arb_member_filter(), 0..=10usize).prop_filter_map(
        "unique IDs required",
        |filters| {
            let ids: std::collections::HashSet<&str> =
                filters.iter().map(mqoguard_filter_bind_report::MqoFilter::filter_id).collect();
            if ids.len() == filters.len() {
                Some(BoundMqo {
                    filters,
                    catalog: None,
                })
            } else {
                None
            }
        },
    )
}

fn arb_compiled_query() -> impl Strategy<Value = CompiledQuery> {
    "[a-z0-9 '=_.-]{0,300}".prop_map(|sql| CompiledQuery {
        sql,
        bound_filter_ids: None,
    })
}

proptest! {
    /// Partition is total: every input filter is accounted for exactly once.
    #[test]
    fn prop_partition_is_total(
        mqo in arb_bound_mqo(),
        compiled in arb_compiled_query(),
    ) {
        let input_count = mqo.filters.len();
        let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);

        prop_assert_eq!(report.input_filter_count, input_count);
        prop_assert_eq!(
            report.applied.len() + report.dropped.len(),
            input_count,
        );
    }

    /// No filter appears in both applied and dropped.
    #[test]
    fn prop_no_filter_in_both_lists(
        mqo in arb_bound_mqo(),
        compiled in arb_compiled_query(),
    ) {
        let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);

        let applied_ids: std::collections::HashSet<String> = report
            .applied
            .iter()
            .map(|f| f.filter.filter_id().to_owned())
            .collect();
        for df in &report.dropped {
            prop_assert!(
                !applied_ids.contains(df.filter.filter_id()),
                "filter {} appears in both applied and dropped",
                df.filter.filter_id()
            );
        }
    }

    /// report_filters is deterministic: calling it twice yields identical results.
    #[test]
    fn prop_deterministic(
        mqo in arb_bound_mqo(),
        compiled in arb_compiled_query(),
    ) {
        let r1 = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);
        let r2 = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);

        let Ok(r1_json) = serde_json::to_string(&r1) else {
            // serde_json only fails on maps with non-string keys; can't happen here.
            return Ok(());
        };
        let Ok(r2_json) = serde_json::to_string(&r2) else {
            return Ok(());
        };
        prop_assert_eq!(r1_json, r2_json);
    }
}
