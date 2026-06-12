// AC7: agentns::read_self() returns Unsupported on this host (no /proc/self/agent_counters)
//   without erroring, and callers can fall back to the userspace ledger.

use mcp_query_budget_governor::agentns::{read_self, CountersOutcome};
use mcp_query_budget_governor::{BudgetLedger, BudgetLimits, Verdict};

#[test]
fn read_self_returns_unsupported_on_macos() {
    let outcome = read_self();
    // On macOS, /proc/self/agent_counters does not exist.
    // The function must return Unsupported without panicking or returning an error.
    assert!(
        matches!(outcome, CountersOutcome::Unsupported),
        "expected Unsupported on macOS (no /proc/self/agent_counters)"
    );
}

#[test]
fn caller_falls_back_to_userspace_ledger() {
    // Simulate the typical caller pattern: try agentns, fall back to ledger.
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
        ledger.record_query(100, 50);
    }

    let outcome = read_self();
    let verdict = match outcome {
        CountersOutcome::Counters(_) => {
            // Would use kernel counters if available; here we still delegate to ledger check.
            ledger.check(0)
        }
        CountersOutcome::Unsupported => {
            // Fallback path: use userspace ledger.
            ledger.check(0)
        }
    };

    // 5/10 queries = 50% — should Proceed.
    assert!(
        matches!(verdict, Verdict::Proceed),
        "fallback to userspace ledger should Proceed at 50%, got {:?}",
        verdict
    );
}

#[test]
fn unsupported_does_not_panic_called_many_times() {
    // Ensure no panic or abort on repeated calls.
    for _ in 0..100 {
        let outcome = read_self();
        assert!(matches!(outcome, CountersOutcome::Unsupported));
    }
}
