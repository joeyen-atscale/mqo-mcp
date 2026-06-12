// AC5: max_wall_ms triggers based on now_ms - started_ms independent of query count
//   (a long-idle loop still halts on wall-clock).

use mcp_query_budget_governor::{BudgetLedger, BudgetLimits, Verdict};

fn make_wall_ledger(max_wall_ms: u64, checkin_fraction: f64, started_ms: u64) -> BudgetLedger {
    BudgetLedger::new(
        BudgetLimits {
            max_queries: None,
            max_est_tokens: None,
            max_latency_ms: None,
            max_wall_ms: Some(max_wall_ms),
            checkin_fraction,
        },
        started_ms,
    )
}

#[test]
fn proceed_before_wall_limit() {
    let ledger = make_wall_ledger(10_000, 0.8, 0);
    // 5s elapsed out of 10s limit = 50%
    let verdict = ledger.check(5_000);
    assert!(
        matches!(verdict, Verdict::Proceed),
        "expected Proceed at 50% wall time, got {:?}",
        verdict
    );
}

#[test]
fn checkin_at_wall_limit_checkin_fraction() {
    let ledger = make_wall_ledger(10_000, 0.8, 0);
    // 8s elapsed = 80% of 10s limit
    let verdict = ledger.check(8_000);
    assert!(
        matches!(verdict, Verdict::CheckIn { .. }),
        "expected CheckIn at 80% wall time, got {:?}",
        verdict
    );
}

#[test]
fn halt_at_wall_limit() {
    let ledger = make_wall_ledger(10_000, 0.8, 0);
    // 10s elapsed = 100% = at the limit
    let verdict = ledger.check(10_000);
    assert!(
        matches!(verdict, Verdict::Halt { .. }),
        "expected Halt at 100% wall time, got {:?}",
        verdict
    );
}

#[test]
fn halt_above_wall_limit() {
    let ledger = make_wall_ledger(10_000, 0.8, 0);
    // 15s elapsed = 150% — well past limit
    let verdict = ledger.check(15_000);
    assert!(
        matches!(verdict, Verdict::Halt { .. }),
        "expected Halt past wall limit, got {:?}",
        verdict
    );
}

#[test]
fn wall_halt_with_zero_queries_idle_loop() {
    // Key AC5 scenario: no queries run, but wall clock says stop.
    let ledger = make_wall_ledger(5_000, 0.8, 1_000);
    // started_ms=1000, now_ms=6500, elapsed=5500 > 5000 limit
    let verdict = ledger.check(6_500);
    assert!(
        matches!(verdict, Verdict::Halt { .. }),
        "expected Halt for idle loop past wall limit, got {:?}",
        verdict
    );
    assert_eq!(ledger.queries_run, 0, "no queries should have been run");
}

#[test]
fn wall_limit_ignores_query_count() {
    let mut ledger = make_wall_ledger(10_000, 0.8, 0);
    // Run many queries but we check before the wall limit
    for _ in 0..1000 {
        ledger.record_query(0, 0);
    }
    // 7s elapsed = 70% — still proceed
    let verdict = ledger.check(7_000);
    assert!(
        matches!(verdict, Verdict::Proceed),
        "expected Proceed at 70% wall time despite many queries, got {:?}",
        verdict
    );
}
