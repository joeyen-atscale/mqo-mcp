// AC4: ingest_audit_log sums latency_ms and counts records from a JSONL file;
//   a corrupt line is skipped with a stderr warning (read path must not abort).

use mcp_query_budget_governor::{BudgetLedger, BudgetLimits, Verdict};
use std::io::Write;

fn make_temp_jsonl(tag: &str, lines: &[&str]) -> std::path::PathBuf {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("ac4_audit_{}_{}.jsonl", std::process::id(), tag));
    let mut f = std::fs::File::create(&path).unwrap();
    for line in lines {
        writeln!(f, "{}", line).unwrap();
    }
    path
}

#[test]
fn ingest_sums_latency_and_counts_records() {
    let path = make_temp_jsonl("sums", &[
        r#"{"query_id":"q1","latency_ms":100,"tokens":50}"#,
        r#"{"query_id":"q2","latency_ms":200,"tokens":80}"#,
        r#"{"query_id":"q3","latency_ms":150,"tokens":60}"#,
    ]);

    let mut ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: Some(20),
            max_est_tokens: None,
            max_latency_ms: Some(1000),
            max_wall_ms: None,
            checkin_fraction: 0.8,
        },
        0,
    );

    let count = ledger.ingest_audit_log(&path).unwrap();
    assert_eq!(count, 3, "expected 3 records ingested");
    assert_eq!(ledger.queries_run, 3);
    assert_eq!(ledger.total_latency_ms, 450); // 100+200+150

    // 450/1000 = 45% — below checkin
    let verdict = ledger.check(0);
    assert!(
        matches!(verdict, Verdict::Proceed),
        "expected Proceed at 45% latency, got {:?}",
        verdict
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn ingest_skips_corrupt_lines_without_abort() {
    let path = make_temp_jsonl("corrupt", &[
        r#"{"query_id":"q1","latency_ms":300}"#,
        r#"THIS IS NOT JSON {{{{"#,           // corrupt
        r#"{"query_id":"q3","latency_ms":200}"#,
        r#"also bad"#,                         // corrupt (bare string)
        r#"{"query_id":"q5","latency_ms":100}"#,
    ]);

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

    let count = ledger.ingest_audit_log(&path).unwrap();
    assert_eq!(count, 3, "expected 3 valid records; corrupt lines skipped");
    assert_eq!(ledger.total_latency_ms, 600); // 300+200+100

    let _ = std::fs::remove_file(path);
}

#[test]
fn ingest_post_spend_causes_checkin() {
    // Start with some latency already, then ingest pushes us into CheckIn territory.
    let path = make_temp_jsonl("checkin", &[
        r#"{"latency_ms":400}"#,
        r#"{"latency_ms":400}"#,
    ]);

    let mut ledger = BudgetLedger::new(
        BudgetLimits {
            max_queries: None,
            max_est_tokens: None,
            max_latency_ms: Some(1000),
            max_wall_ms: None,
            checkin_fraction: 0.8,
        },
        0,
    );

    ledger.ingest_audit_log(&path).unwrap(); // 800ms = 80% of 1000
    let verdict = ledger.check(0);
    assert!(
        matches!(verdict, Verdict::CheckIn { .. }),
        "expected CheckIn after ingest pushing to 80%, got {:?}",
        verdict
    );

    let _ = std::fs::remove_file(path);
}

#[test]
fn ingest_missing_latency_field_counts_record_with_zero_latency() {
    // Records without a latency_ms field should still be counted.
    let path = make_temp_jsonl("nolat", &[
        r#"{"query_id":"q1"}"#,
        r#"{"query_id":"q2","latency_ms":500}"#,
    ]);

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

    let count = ledger.ingest_audit_log(&path).unwrap();
    assert_eq!(count, 2, "both records counted");
    assert_eq!(ledger.queries_run, 2);
    assert_eq!(ledger.total_latency_ms, 500); // only q2 has latency

    let _ = std::fs::remove_file(path);
}
