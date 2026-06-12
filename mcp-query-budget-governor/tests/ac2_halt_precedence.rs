// AC2: Halt dominates: if max_latency_ms is exceeded but max_queries is only at 50%,
//   the verdict is Halt naming the latency limit.

use mcp_query_budget_governor::{BudgetLedger, BudgetLimits, Verdict};

#[test]
fn halt_names_latency_limit_when_queries_at_50_percent() {
    let mut ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: Some(10),       // limit = 10
            max_est_tokens: None,
            max_latency_ms: Some(1000),  // limit = 1000 ms
            max_wall_ms: None,
            checkin_fraction: 0.8,
        },
        0,
    );

    // 5 queries = 50% of max_queries (well below checkin and halt)
    // 1500 ms latency = 150% of max_latency_ms (over hard limit)
    for _ in 0..5 {
        ledger.record_query(0, 300); // 5 * 300 = 1500 ms total
    }

    let verdict = ledger.check(0);
    match verdict {
        Verdict::Halt { ref reason, ref limit } => {
            // The limit string should reference the latency limit
            assert!(
                limit.contains("ms latency") || reason.contains("latency"),
                "expected latency named in Halt, got limit={:?} reason={:?}",
                limit,
                reason
            );
        }
        other => panic!("expected Halt, got {:?}", other),
    }
}

#[test]
fn checkin_on_latency_when_queries_are_fine() {
    let mut ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: Some(10),
            max_est_tokens: None,
            max_latency_ms: Some(1000),
            max_wall_ms: None,
            checkin_fraction: 0.8,
        },
        0,
    );

    // 3 queries at 280ms = 840ms total = 84% of 1000ms (triggers CheckIn)
    // queries = 3/10 = 30% (below checkin at 80%)
    for _ in 0..3 {
        ledger.record_query(0, 280);
    }

    let verdict = ledger.check(0);
    assert!(
        matches!(verdict, Verdict::CheckIn { .. }),
        "expected CheckIn when latency at 84%, got {:?}",
        verdict
    );
}
