// AC6: check is pure w.r.t. time — identical ledger + same now_ms yields identical
//   Verdict across calls, no I/O.

use mcp_query_budget_governor::{BudgetLedger, BudgetLimits, Verdict};

fn verdict_tag(v: &Verdict) -> &'static str {
    match v {
        Verdict::Proceed => "Proceed",
        Verdict::CheckIn { .. } => "CheckIn",
        Verdict::Halt { .. } => "Halt",
    }
}

#[test]
fn check_is_idempotent_proceed() {
    let mut ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: Some(10),
            max_est_tokens: None,
            max_latency_ms: None,
            max_wall_ms: None,
            checkin_fraction: 0.8,
        },
        0,
    );
    ledger.record_query(0, 0); // 1/10

    let now_ms = 12345;
    let v1 = ledger.check(now_ms);
    let v2 = ledger.check(now_ms);
    let v3 = ledger.check(now_ms);

    assert_eq!(verdict_tag(&v1), "Proceed");
    assert_eq!(verdict_tag(&v2), "Proceed");
    assert_eq!(verdict_tag(&v3), "Proceed");
}

#[test]
fn check_is_idempotent_checkin() {
    let mut ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: Some(10),
            max_est_tokens: None,
            max_latency_ms: None,
            max_wall_ms: None,
            checkin_fraction: 0.8,
        },
        0,
    );
    for _ in 0..8 {
        ledger.record_query(0, 0); // 8/10 = 80%
    }

    let now_ms = 99999;
    for _ in 0..10 {
        let v = ledger.check(now_ms);
        assert_eq!(
            verdict_tag(&v),
            "CheckIn",
            "repeated check should always return CheckIn"
        );
    }
}

#[test]
fn check_is_idempotent_halt() {
    let mut ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: Some(5),
            max_est_tokens: None,
            max_latency_ms: None,
            max_wall_ms: None,
            checkin_fraction: 0.8,
        },
        0,
    );
    for _ in 0..5 {
        ledger.record_query(0, 0); // 5/5 = 100%
    }

    let now_ms = 0;
    for _ in 0..10 {
        let v = ledger.check(now_ms);
        assert_eq!(
            verdict_tag(&v),
            "Halt",
            "repeated check should always return Halt"
        );
    }
}

#[test]
fn same_now_ms_same_verdict() {
    // Demonstrate that check does not depend on real wall clock (only now_ms).
    let mut ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: None,
            max_est_tokens: None,
            max_latency_ms: None,
            max_wall_ms: Some(10_000),
            checkin_fraction: 0.8,
        },
        0,
    );
    ledger.record_query(0, 0);

    // Same now_ms → same verdict regardless of when the call actually happens.
    let v1 = ledger.check(5_000);
    let v2 = ledger.check(5_000);
    assert_eq!(verdict_tag(&v1), verdict_tag(&v2));

    // Different now_ms → different verdict
    let v_before = ledger.check(5_000); // 50% wall — Proceed
    let v_after = ledger.check(9_000);  // 90% wall — CheckIn

    assert_eq!(verdict_tag(&v_before), "Proceed");
    assert_eq!(verdict_tag(&v_after), "CheckIn");
}

#[test]
fn check_does_not_mutate_ledger() {
    let mut ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: Some(10),
            max_est_tokens: None,
            max_latency_ms: None,
            max_wall_ms: None,
            checkin_fraction: 0.8,
        },
        0,
    );
    for _ in 0..5 {
        ledger.record_query(10, 100);
    }

    let queries_before = ledger.queries_run;
    let tokens_before = ledger.est_tokens;
    let latency_before = ledger.total_latency_ms;

    ledger.check(0);
    ledger.check(1000);
    ledger.check(2000);

    assert_eq!(ledger.queries_run, queries_before, "check must not mutate queries_run");
    assert_eq!(ledger.est_tokens, tokens_before, "check must not mutate est_tokens");
    assert_eq!(ledger.total_latency_ms, latency_before, "check must not mutate total_latency_ms");
}
