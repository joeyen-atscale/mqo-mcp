// AC1: With max_queries=10, checkin_fraction=0.8,
//   check returns Proceed at 7 queries, CheckIn at 8 (80%), and Halt at 10.

use mcp_query_budget_governor::{BudgetLedger, BudgetLimits, Verdict};

fn make_ledger(max_queries: u64, checkin_fraction: f64) -> BudgetLedger {
    BudgetLedger::new(
        BudgetLimits {
            max_queries: Some(max_queries),
            max_est_tokens: None,
            max_latency_ms: None,
            max_wall_ms: None,
            checkin_fraction,
        },
        0,
    )
}

#[test]
fn proceed_at_7_queries() {
    let mut ledger = make_ledger(10, 0.8);
    for _ in 0..7 {
        ledger.record_query(0, 0);
    }
    let verdict = ledger.check(0);
    assert!(
        matches!(verdict, Verdict::Proceed),
        "expected Proceed at 7/10 queries, got {:?}",
        verdict
    );
}

#[test]
fn checkin_at_8_queries() {
    let mut ledger = make_ledger(10, 0.8);
    for _ in 0..8 {
        ledger.record_query(0, 0);
    }
    let verdict = ledger.check(0);
    assert!(
        matches!(verdict, Verdict::CheckIn { .. }),
        "expected CheckIn at 8/10 queries (80%), got {:?}",
        verdict
    );
    if let Verdict::CheckIn { fraction_used, .. } = verdict {
        assert!(
            (fraction_used - 0.8).abs() < 1e-9,
            "fraction_used should be 0.8, got {}",
            fraction_used
        );
    }
}

#[test]
fn halt_at_10_queries() {
    let mut ledger = make_ledger(10, 0.8);
    for _ in 0..10 {
        ledger.record_query(0, 0);
    }
    let verdict = ledger.check(0);
    assert!(
        matches!(verdict, Verdict::Halt { .. }),
        "expected Halt at 10/10 queries, got {:?}",
        verdict
    );
}

#[test]
fn halt_above_10_queries() {
    let mut ledger = make_ledger(10, 0.8);
    for _ in 0..12 {
        ledger.record_query(0, 0);
    }
    let verdict = ledger.check(0);
    assert!(
        matches!(verdict, Verdict::Halt { .. }),
        "expected Halt above limit, got {:?}",
        verdict
    );
}
