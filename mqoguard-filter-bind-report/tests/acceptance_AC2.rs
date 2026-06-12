//! AC2: Given any MQO, applied ∪ dropped equals the input filter set with no
//! duplicates or omissions. Property test.

use mqoguard_filter_bind_report::{BoundMqo, CompiledQuery, MemberFilter, MqoFilter};
use proptest::prelude::*;

fn arb_member_filter() -> impl Strategy<Value = MqoFilter> {
    (
        "[a-z0-9]{1,8}",
        "[a-z_]{3,20}",
        prop::collection::vec("[a-z0-9]{1,10}", 0..=4usize),
    )
        .prop_map(|(id, hierarchy, members)| {
            MqoFilter::Member(MemberFilter {
                filter_id: id,
                hierarchy,
                level: None,
                members,
            })
        })
}

fn arb_bound_mqo() -> impl Strategy<Value = BoundMqo> {
    prop::collection::vec(arb_member_filter(), 0..=8usize).prop_map(|filters| BoundMqo {
        filters,
        catalog: None,
    })
}

fn arb_compiled_query() -> impl Strategy<Value = CompiledQuery> {
    ("[a-z0-9 '=_]{0,200}", prop::bool::ANY).prop_map(|(sql, use_exact)| {
        let bound_filter_ids = if use_exact {
            Some(vec![]) // empty exact list → all dropped
        } else {
            None
        };
        CompiledQuery {
            sql,
            bound_filter_ids,
        }
    })
}

proptest! {
    /// The applied∪dropped union must exactly equal the input filter set.
    #[test]
    fn prop_applied_union_dropped_equals_input(
        mqo in arb_bound_mqo(),
        compiled in arb_compiled_query(),
    ) {
        let expected_ids: std::collections::HashSet<String> =
            mqo.filters.iter().map(|f| f.filter_id().to_owned()).collect();

        // Only test when filter IDs are unique (skip degenerate duplicates).
        prop_assume!(expected_ids.len() == mqo.filters.len());

        let report = mqoguard_filter_bind_report::report_filters(&mqo, &compiled);

        // Total count invariant.
        prop_assert_eq!(
            report.applied.len() + report.dropped.len(),
            report.input_filter_count,
        );
        prop_assert_eq!(report.input_filter_count, mqo.filters.len());

        // Union of IDs == input IDs, no duplicates.
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for af in &report.applied {
            let id = af.filter.filter_id().to_owned();
            prop_assert!(!seen_ids.contains(&id), "duplicate id in applied: {id}");
            seen_ids.insert(id);
        }
        for df in &report.dropped {
            let id = df.filter.filter_id().to_owned();
            prop_assert!(!seen_ids.contains(&id), "duplicate id in dropped: {id}");
            seen_ids.insert(id);
        }
        prop_assert_eq!(seen_ids, expected_ids);
    }
}
