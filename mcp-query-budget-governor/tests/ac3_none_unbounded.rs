// AC3: A None limit never triggers CheckIn/Halt no matter the accumulated value on that axis.

use mcp_query_budget_governor::{BudgetLedger, BudgetLimits, Verdict};

#[test]
fn no_limits_at_all_always_proceed() {
    let mut ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: None,
            max_est_tokens: None,
            max_latency_ms: None,
            max_wall_ms: None,
            checkin_fraction: 0.8,
        },
        0,
    );

    for _ in 0..10_000 {
        ledger.record_query(100_000, 999_999);
    }

    let verdict = ledger.check(u64::MAX);
    assert!(
        matches!(verdict, Verdict::Proceed),
        "expected Proceed with all-None limits, got {:?}",
        verdict
    );
}

#[test]
fn none_queries_does_not_constrain() {
    let mut ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: None,       // unbounded
            max_est_tokens: None,
            max_latency_ms: Some(10_000), // latency limit present but not exceeded
            max_wall_ms: None,
            checkin_fraction: 0.8,
        },
        0,
    );

    // Run many queries — queries axis is unconstrained
    for _ in 0..100 {
        ledger.record_query(0, 10); // total latency = 1000ms = 10% of 10_000 limit
    }

    let verdict = ledger.check(0);
    assert!(
        matches!(verdict, Verdict::Proceed),
        "expected Proceed when query limit is None, got {:?}",
        verdict
    );
}

#[test]
fn none_tokens_does_not_constrain() {
    let mut ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: None,
            max_est_tokens: None,   // unbounded
            max_latency_ms: None,
            max_wall_ms: None,
            checkin_fraction: 0.5,
        },
        0,
    );

    for _ in 0..100 {
        ledger.record_query(1_000_000, 0);
    }

    let verdict = ledger.check(0);
    assert!(
        matches!(verdict, Verdict::Proceed),
        "expected Proceed when token limit is None, got {:?}",
        verdict
    );
}

#[test]
fn none_wall_does_not_constrain() {
    let ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: None,
            max_est_tokens: None,
            max_latency_ms: None,
            max_wall_ms: None,      // unbounded
            checkin_fraction: 0.8,
        },
        0,
    );

    // Far future now_ms
    let verdict = ledger.check(u64::MAX);
    assert!(
        matches!(verdict, Verdict::Proceed),
        "expected Proceed when wall_ms limit is None, got {:?}",
        verdict
    );
}
