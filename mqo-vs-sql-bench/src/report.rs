//! Report computation and rendering (JSON + Markdown).

use std::fmt::Write as _;

use crate::types::{AggregateReport, QuestionResult};

/// Compute the aggregate report from per-question results.
#[must_use]
pub fn compute_aggregate(results: &[QuestionResult]) -> AggregateReport {
    let n = results.len();
    if n == 0 {
        return AggregateReport {
            n_questions: 0,
            arm_a_pass_count: 0,
            arm_b_pass_count: 0,
            arm_a_accuracy: 0.0,
            arm_b_accuracy: 0.0,
            accuracy_delta: 0.0,
            accuracy_winner: "tie".to_string(),
            arm_a_invalid_entity_count: 0,
            arm_b_invalid_entity_count: 0,
            arm_a_invalid_entity_rate: 0.0,
            arm_b_invalid_entity_rate: 0.0,
            invalid_entity_delta: 0.0,
            invalid_entity_winner: "tie".to_string(),
            arm_a_total_retries: 0,
            arm_b_total_retries: 0,
            arm_a_mean_retries: 0.0,
            arm_b_mean_retries: 0.0,
            retry_delta: 0.0,
            retry_winner: "tie".to_string(),
            arm_a_total_latency_ms: 0,
            arm_b_total_latency_ms: 0,
            arm_a_mean_latency_ms: 0.0,
            arm_b_mean_latency_ms: 0.0,
            latency_delta_ms: 0.0,
            latency_winner: "tie".to_string(),
            arm_a_total_tokens: 0,
            arm_b_total_tokens: 0,
            arm_a_mean_tokens: 0.0,
            arm_b_mean_tokens: 0.0,
            token_delta: 0.0,
            token_winner: "tie".to_string(),
        };
    }

    let nf = n as f64;

    // Accuracy
    let arm_a_pass_count = results.iter().filter(|r| r.arm_a_pass).count();
    let arm_b_pass_count = results.iter().filter(|r| r.arm_b_pass).count();
    let arm_a_accuracy = arm_a_pass_count as f64 / nf;
    let arm_b_accuracy = arm_b_pass_count as f64 / nf;
    let accuracy_delta = arm_b_accuracy - arm_a_accuracy;
    let accuracy_winner = higher_winner(arm_a_accuracy, arm_b_accuracy);

    // Invalid-entity (hallucination) rate — lower is better
    let arm_a_invalid_entity_count = results.iter().filter(|r| r.arm_a.invalid_entity).count();
    let arm_b_invalid_entity_count = results.iter().filter(|r| r.arm_b.invalid_entity).count();
    let arm_a_invalid_entity_rate = arm_a_invalid_entity_count as f64 / nf;
    let arm_b_invalid_entity_rate = arm_b_invalid_entity_count as f64 / nf;
    let invalid_entity_delta = arm_b_invalid_entity_rate - arm_a_invalid_entity_rate;
    let invalid_entity_winner = lower_winner(arm_a_invalid_entity_rate, arm_b_invalid_entity_rate);

    // Retries — lower is better
    let arm_a_total_retries: u64 = results.iter().map(|r| u64::from(r.arm_a.n_retries)).sum();
    let arm_b_total_retries: u64 = results.iter().map(|r| u64::from(r.arm_b.n_retries)).sum();
    let arm_a_mean_retries = arm_a_total_retries as f64 / nf;
    let arm_b_mean_retries = arm_b_total_retries as f64 / nf;
    let retry_delta = arm_b_mean_retries - arm_a_mean_retries;
    let retry_winner = lower_winner(arm_a_mean_retries, arm_b_mean_retries);

    // Latency — lower is better
    let arm_a_total_latency_ms: u64 = results.iter().map(|r| r.arm_a.latency_ms).sum();
    let arm_b_total_latency_ms: u64 = results.iter().map(|r| r.arm_b.latency_ms).sum();
    let arm_a_mean_latency_ms = arm_a_total_latency_ms as f64 / nf;
    let arm_b_mean_latency_ms = arm_b_total_latency_ms as f64 / nf;
    let latency_delta_ms = arm_b_mean_latency_ms - arm_a_mean_latency_ms;
    let latency_winner = lower_winner(arm_a_mean_latency_ms, arm_b_mean_latency_ms);

    // Tokens — lower is better
    let arm_a_total_tokens: u64 = results.iter().map(|r| r.arm_a.tokens).sum();
    let arm_b_total_tokens: u64 = results.iter().map(|r| r.arm_b.tokens).sum();
    let arm_a_mean_tokens = arm_a_total_tokens as f64 / nf;
    let arm_b_mean_tokens = arm_b_total_tokens as f64 / nf;
    let token_delta = arm_b_mean_tokens - arm_a_mean_tokens;
    let token_winner = lower_winner(arm_a_mean_tokens, arm_b_mean_tokens);

    AggregateReport {
        n_questions: n,
        arm_a_pass_count,
        arm_b_pass_count,
        arm_a_accuracy,
        arm_b_accuracy,
        accuracy_delta,
        accuracy_winner,
        arm_a_invalid_entity_count,
        arm_b_invalid_entity_count,
        arm_a_invalid_entity_rate,
        arm_b_invalid_entity_rate,
        invalid_entity_delta,
        invalid_entity_winner,
        arm_a_total_retries,
        arm_b_total_retries,
        arm_a_mean_retries,
        arm_b_mean_retries,
        retry_delta,
        retry_winner,
        arm_a_total_latency_ms,
        arm_b_total_latency_ms,
        arm_a_mean_latency_ms,
        arm_b_mean_latency_ms,
        latency_delta_ms,
        latency_winner,
        arm_a_total_tokens,
        arm_b_total_tokens,
        arm_a_mean_tokens,
        arm_b_mean_tokens,
        token_delta,
        token_winner,
    }
}

/// Return `"arm_a_sql"` / `"arm_b_mqo"` / `"tie"` for a metric where higher = better.
fn higher_winner(a: f64, b: f64) -> String {
    if (b - a).abs() < 1e-9 {
        "tie".to_string()
    } else if b > a {
        "arm_b_mqo".to_string()
    } else {
        "arm_a_sql".to_string()
    }
}

/// Return `"arm_a_sql"` / `"arm_b_mqo"` / `"tie"` for a metric where lower = better.
fn lower_winner(a: f64, b: f64) -> String {
    if (b - a).abs() < 1e-9 {
        "tie".to_string()
    } else if b < a {
        "arm_b_mqo".to_string()
    } else {
        "arm_a_sql".to_string()
    }
}

/// Render the full report as a Markdown string.
#[must_use]
pub fn render_markdown(results: &[QuestionResult], agg: &AggregateReport) -> String {
    let mut md = String::new();

    md.push_str("# MQO vs SQL Benchmark Report\n\n");

    // Aggregate summary table
    md.push_str("## Aggregate Summary\n\n");
    md.push_str("| Metric | arm_a_sql | arm_b_mqo | Delta (B−A) | Winner |\n");
    md.push_str("|--------|-----------|-----------|-------------|--------|\n");

    let _ = writeln!(
        md,
        "| Accuracy | {:.1}% ({}/{}) | {:.1}% ({}/{}) | {:+.1}% | **{}** |",
        agg.arm_a_accuracy * 100.0,
        agg.arm_a_pass_count,
        agg.n_questions,
        agg.arm_b_accuracy * 100.0,
        agg.arm_b_pass_count,
        agg.n_questions,
        agg.accuracy_delta * 100.0,
        agg.accuracy_winner
    );

    let _ = writeln!(
        md,
        "| Invalid-entity rate | {:.1}% | {:.1}% | {:+.1}% | **{}** |",
        agg.arm_a_invalid_entity_rate * 100.0,
        agg.arm_b_invalid_entity_rate * 100.0,
        agg.invalid_entity_delta * 100.0,
        agg.invalid_entity_winner
    );

    let _ = writeln!(
        md,
        "| Mean retries | {:.2} | {:.2} | {:+.2} | **{}** |",
        agg.arm_a_mean_retries, agg.arm_b_mean_retries, agg.retry_delta, agg.retry_winner
    );

    let _ = writeln!(
        md,
        "| Mean latency (ms) | {:.1} | {:.1} | {:+.1} | **{}** |",
        agg.arm_a_mean_latency_ms,
        agg.arm_b_mean_latency_ms,
        agg.latency_delta_ms,
        agg.latency_winner
    );

    let _ = writeln!(
        md,
        "| Mean tokens | {:.1} | {:.1} | {:+.1} | **{}** |",
        agg.arm_a_mean_tokens, agg.arm_b_mean_tokens, agg.token_delta, agg.token_winner
    );

    md.push('\n');

    // Per-question table
    md.push_str("## Per-Question Results\n\n");
    md.push_str(
        "| Task | Question | A pass | B pass | Equivalent | A retries | B retries | A latency ms | B latency ms |\n",
    );
    md.push_str(
        "|------|----------|--------|--------|------------|-----------|-----------|-------------|-------------|\n",
    );

    for r in results {
        let q = r.question.chars().take(60).collect::<String>();
        let q = if r.question.len() > 60 {
            format!("{q}…")
        } else {
            q
        };
        let _ = writeln!(
            md,
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            r.task_id,
            q,
            if r.arm_a_pass { "✓" } else { "✗" },
            if r.arm_b_pass { "✓" } else { "✗" },
            if r.arms_equivalent { "✓" } else { "✗" },
            r.arm_a.n_retries,
            r.arm_b.n_retries,
            r.arm_a.latency_ms,
            r.arm_b.latency_ms,
        );
    }

    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Arm, ArmOutput, QuestionResult};

    fn make_arm(arm: Arm, _pass: bool, invalid_entity: bool, retries: u32, latency_ms: u64, tokens: u64) -> ArmOutput {
        ArmOutput {
            arm,
            query: "SELECT 1".to_string(),
            rows: vec![],
            error: None,
            n_retries: retries,
            latency_ms,
            tokens,
            invalid_entity,
        }
    }

    fn make_result(id: &str, a_pass: bool, b_pass: bool) -> QuestionResult {
        QuestionResult {
            task_id: id.to_string(),
            question: format!("Question {id}"),
            arm_a: make_arm(Arm::SqlRunQuery, a_pass, false, 0, 100, 500),
            arm_b: make_arm(Arm::MqoMultidimensional, b_pass, false, 0, 80, 400),
            arms_equivalent: a_pass == b_pass,
            arm_a_pass: a_pass,
            arm_b_pass: b_pass,
            grader_reason: None,
        }
    }

    #[test]
    fn test_compute_aggregate_empty() {
        let agg = compute_aggregate(&[]);
        assert_eq!(agg.n_questions, 0);
        assert_eq!(agg.accuracy_winner, "tie");
    }

    #[test]
    fn test_compute_aggregate_all_pass() {
        let results = vec![
            make_result("t1", true, true),
            make_result("t2", true, true),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.n_questions, 2);
        assert!((agg.arm_a_accuracy - 1.0).abs() < 1e-9);
        assert!((agg.arm_b_accuracy - 1.0).abs() < 1e-9);
        assert_eq!(agg.accuracy_winner, "tie");
    }

    #[test]
    fn test_compute_aggregate_b_wins_accuracy() {
        let results = vec![
            make_result("t1", false, true),
            make_result("t2", false, true),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.accuracy_winner, "arm_b_mqo");
        assert!((agg.arm_a_accuracy - 0.0).abs() < 1e-9);
        assert!((agg.arm_b_accuracy - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_latency_winner_lower_is_better() {
        // arm_b has lower latency (80ms) → arm_b wins
        let results = vec![make_result("t1", true, true)];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.latency_winner, "arm_b_mqo");
    }

    #[test]
    fn test_render_markdown_contains_headers() {
        let results = vec![make_result("t1", true, false)];
        let agg = compute_aggregate(&results);
        let md = render_markdown(&results, &agg);
        assert!(md.contains("# MQO vs SQL Benchmark Report"));
        assert!(md.contains("## Aggregate Summary"));
        assert!(md.contains("## Per-Question Results"));
        assert!(md.contains("arm_a_sql"));
        assert!(md.contains("arm_b_mqo"));
    }

    // ── Targeted arithmetic tests to kill surviving mutants ─────────────────

    fn make_arm_full(
        arm: Arm,
        invalid_entity: bool,
        retries: u32,
        latency_ms: u64,
        tokens: u64,
    ) -> ArmOutput {
        ArmOutput {
            arm,
            query: "Q".to_string(),
            rows: vec![],
            error: None,
            n_retries: retries,
            latency_ms,
            tokens,
            invalid_entity,
        }
    }

    fn make_result_full(
        id: &str,
        a_pass: bool,
        b_pass: bool,
        a_invalid: bool,
        b_invalid: bool,
        a_retries: u32,
        b_retries: u32,
        a_latency: u64,
        b_latency: u64,
        a_tokens: u64,
        b_tokens: u64,
    ) -> QuestionResult {
        QuestionResult {
            task_id: id.to_string(),
            question: format!("Q {id}"),
            arm_a: make_arm_full(Arm::SqlRunQuery, a_invalid, a_retries, a_latency, a_tokens),
            arm_b: make_arm_full(Arm::MqoMultidimensional, b_invalid, b_retries, b_latency, b_tokens),
            arms_equivalent: a_pass == b_pass,
            arm_a_pass: a_pass,
            arm_b_pass: b_pass,
            grader_reason: None,
        }
    }

    /// Arm A wins accuracy (arm_a has more passes).
    #[test]
    fn test_accuracy_arm_a_wins() {
        let results = vec![
            make_result("t1", true, false),
            make_result("t2", true, false),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.accuracy_winner, "arm_a_sql");
        assert_eq!(agg.arm_a_pass_count, 2);
        assert_eq!(agg.arm_b_pass_count, 0);
        // accuracy_delta = arm_b - arm_a = 0 - 1 = -1
        assert!((agg.accuracy_delta - (-1.0_f64)).abs() < 1e-9);
    }

    /// Pass counts are counted independently per arm.
    #[test]
    fn test_pass_counts_exact() {
        let results = vec![
            make_result("t1", true, false),
            make_result("t2", false, true),
            make_result("t3", true, true),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.n_questions, 3);
        assert_eq!(agg.arm_a_pass_count, 2);
        assert_eq!(agg.arm_b_pass_count, 2);
        assert_eq!(agg.accuracy_winner, "tie");
    }

    /// Invalid-entity counts: arm_a wins lower-is-better when arm_b has more.
    #[test]
    fn test_invalid_entity_arm_a_wins() {
        let results = vec![
            make_result_full("t1", true, true, false, true, 0, 0, 100, 100, 500, 500),
            make_result_full("t2", true, true, false, true, 0, 0, 100, 100, 500, 500),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.arm_a_invalid_entity_count, 0);
        assert_eq!(agg.arm_b_invalid_entity_count, 2);
        assert!((agg.arm_a_invalid_entity_rate - 0.0).abs() < 1e-9);
        assert!((agg.arm_b_invalid_entity_rate - 1.0).abs() < 1e-9);
        assert_eq!(agg.invalid_entity_winner, "arm_a_sql");
        // delta = b - a = 1.0 - 0.0 = 1.0
        assert!((agg.invalid_entity_delta - 1.0_f64).abs() < 1e-9);
    }

    /// Retry totals and means: arm_a has retries, arm_b has none → arm_b wins.
    #[test]
    fn test_retry_counts_and_winner() {
        let results = vec![
            make_result_full("t1", true, true, false, false, 2, 0, 100, 100, 500, 500),
            make_result_full("t2", true, true, false, false, 4, 1, 100, 100, 500, 500),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.arm_a_total_retries, 6);
        assert_eq!(agg.arm_b_total_retries, 1);
        assert!((agg.arm_a_mean_retries - 3.0_f64).abs() < 1e-9);
        assert!((agg.arm_b_mean_retries - 0.5_f64).abs() < 1e-9);
        // retry_delta = b - a = 0.5 - 3.0 = -2.5
        assert!((agg.retry_delta - (-2.5_f64)).abs() < 1e-9);
        assert_eq!(agg.retry_winner, "arm_b_mqo");
    }

    /// Arm A wins retries when arm_b has more.
    #[test]
    fn test_retry_arm_a_wins() {
        let results = vec![
            make_result_full("t1", true, true, false, false, 0, 5, 100, 100, 500, 500),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.retry_winner, "arm_a_sql");
    }

    /// Latency totals and means.
    #[test]
    fn test_latency_totals_and_means() {
        let results = vec![
            make_result_full("t1", true, true, false, false, 0, 0, 200, 100, 500, 500),
            make_result_full("t2", true, true, false, false, 0, 0, 400, 100, 500, 500),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.arm_a_total_latency_ms, 600);
        assert_eq!(agg.arm_b_total_latency_ms, 200);
        assert!((agg.arm_a_mean_latency_ms - 300.0_f64).abs() < 1e-9);
        assert!((agg.arm_b_mean_latency_ms - 100.0_f64).abs() < 1e-9);
        // latency_delta = b - a = 100 - 300 = -200
        assert!((agg.latency_delta_ms - (-200.0_f64)).abs() < 1e-9);
        assert_eq!(agg.latency_winner, "arm_b_mqo");
    }

    /// Arm A wins latency when arm_b is slower.
    #[test]
    fn test_latency_arm_a_wins() {
        let results = vec![
            make_result_full("t1", true, true, false, false, 0, 0, 50, 200, 500, 500),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.latency_winner, "arm_a_sql");
    }

    /// Token totals and means.
    #[test]
    fn test_token_totals_and_means() {
        let results = vec![
            make_result_full("t1", true, true, false, false, 0, 0, 100, 100, 1000, 400),
            make_result_full("t2", true, true, false, false, 0, 0, 100, 100, 2000, 600),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.arm_a_total_tokens, 3000);
        assert_eq!(agg.arm_b_total_tokens, 1000);
        assert!((agg.arm_a_mean_tokens - 1500.0_f64).abs() < 1e-9);
        assert!((agg.arm_b_mean_tokens - 500.0_f64).abs() < 1e-9);
        // token_delta = b - a = 500 - 1500 = -1000
        assert!((agg.token_delta - (-1000.0_f64)).abs() < 1e-9);
        assert_eq!(agg.token_winner, "arm_b_mqo");
    }

    /// Arm A wins tokens when arm_b uses more.
    #[test]
    fn test_token_arm_a_wins() {
        let results = vec![
            make_result_full("t1", true, true, false, false, 0, 0, 100, 100, 100, 9999),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.token_winner, "arm_a_sql");
    }

    /// All metrics tie when both arms are identical.
    #[test]
    fn test_all_metrics_tie() {
        let results = vec![
            make_result_full("t1", true, true, false, false, 1, 1, 150, 150, 700, 700),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.accuracy_winner, "tie");
        assert_eq!(agg.invalid_entity_winner, "tie");
        assert_eq!(agg.retry_winner, "tie");
        assert_eq!(agg.latency_winner, "tie");
        assert_eq!(agg.token_winner, "tie");
    }

    /// Markdown per-question table truncates long questions at 60 chars.
    #[test]
    fn test_markdown_question_truncation() {
        let long_q = "A".repeat(80);
        let result = QuestionResult {
            task_id: "t1".to_string(),
            question: long_q.clone(),
            arm_a: make_arm_full(Arm::SqlRunQuery, false, 0, 10, 100),
            arm_b: make_arm_full(Arm::MqoMultidimensional, false, 0, 10, 100),
            arms_equivalent: true,
            arm_a_pass: true,
            arm_b_pass: true,
            grader_reason: None,
        };
        let agg = compute_aggregate(&[result.clone()]);
        let md = render_markdown(&[result], &agg);
        // The 80-char question should appear truncated with ellipsis.
        assert!(md.contains('…'), "long questions must be truncated with ellipsis");
        // Must not contain the full 80-char string verbatim.
        assert!(!md.contains(&long_q), "full long question must not appear verbatim");
    }

    /// Markdown table marks passing arms with checkmarks and failing with x.
    #[test]
    fn test_markdown_pass_fail_symbols() {
        let result = QuestionResult {
            task_id: "t1".to_string(),
            question: "q".to_string(),
            arm_a: make_arm_full(Arm::SqlRunQuery, false, 0, 10, 100),
            arm_b: make_arm_full(Arm::MqoMultidimensional, false, 0, 10, 100),
            arms_equivalent: false,
            arm_a_pass: true,
            arm_b_pass: false,
            grader_reason: None,
        };
        let agg = compute_aggregate(&[result.clone()]);
        let md = render_markdown(&[result], &agg);
        assert!(md.contains('✓'), "passing arm must show checkmark");
        assert!(md.contains('✗'), "failing arm must show x-mark");
    }

    // ── higher_winner / lower_winner boundary tests ───────────────────────────

    /// higher_winner: b > a → arm_b_mqo.
    #[test]
    fn test_higher_winner_b_greater() {
        assert_eq!(higher_winner(0.5, 0.9), "arm_b_mqo");
    }

    /// higher_winner: a > b → arm_a_sql.
    #[test]
    fn test_higher_winner_a_greater() {
        assert_eq!(higher_winner(0.9, 0.5), "arm_a_sql");
    }

    /// higher_winner: exact equality → tie.
    #[test]
    fn test_higher_winner_tie() {
        assert_eq!(higher_winner(0.5, 0.5), "tie");
    }

    /// lower_winner: b < a → arm_b_mqo.
    #[test]
    fn test_lower_winner_b_less() {
        assert_eq!(lower_winner(0.9, 0.1), "arm_b_mqo");
    }

    /// lower_winner: a < b → arm_a_sql.
    #[test]
    fn test_lower_winner_a_less() {
        assert_eq!(lower_winner(0.1, 0.9), "arm_a_sql");
    }

    /// lower_winner: equal → tie.
    #[test]
    fn test_lower_winner_tie() {
        assert_eq!(lower_winner(0.5, 0.5), "tie");
    }

    /// compute_aggregate accuracy rates are exact fractions.
    #[test]
    fn test_accuracy_rates_exact() {
        // 1 of 3 tasks pass for arm_a; 2 of 3 for arm_b
        let results = vec![
            make_result("t1", true, true),
            make_result("t2", false, true),
            make_result("t3", false, false),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.arm_a_pass_count, 1);
        assert_eq!(agg.arm_b_pass_count, 2);
        // 1/3 ≈ 0.333..., 2/3 ≈ 0.666...
        assert!((agg.arm_a_accuracy - 1.0 / 3.0).abs() < 1e-9);
        assert!((agg.arm_b_accuracy - 2.0 / 3.0).abs() < 1e-9);
    }

    /// Question of exactly 60 chars must NOT be truncated.
    #[test]
    fn test_markdown_question_60_chars_not_truncated() {
        let q60 = "A".repeat(60);
        let result = QuestionResult {
            task_id: "t1".to_string(),
            question: q60.clone(),
            arm_a: make_arm_full(Arm::SqlRunQuery, false, 0, 10, 100),
            arm_b: make_arm_full(Arm::MqoMultidimensional, false, 0, 10, 100),
            arms_equivalent: true,
            arm_a_pass: true,
            arm_b_pass: true,
            grader_reason: None,
        };
        let agg = compute_aggregate(&[result.clone()]);
        let md = render_markdown(&[result], &agg);
        // Exactly 60 chars → no ellipsis appended.
        assert!(!md.contains('…'), "exactly-60-char question must not be truncated");
        assert!(md.contains(&q60), "the 60-char question must appear verbatim");
    }

    /// Question of exactly 61 chars IS truncated.
    #[test]
    fn test_markdown_question_61_chars_is_truncated() {
        let q61 = "B".repeat(61);
        let result = QuestionResult {
            task_id: "t1".to_string(),
            question: q61.clone(),
            arm_a: make_arm_full(Arm::SqlRunQuery, false, 0, 10, 100),
            arm_b: make_arm_full(Arm::MqoMultidimensional, false, 0, 10, 100),
            arms_equivalent: true,
            arm_a_pass: true,
            arm_b_pass: true,
            grader_reason: None,
        };
        let agg = compute_aggregate(&[result.clone()]);
        let md = render_markdown(&[result], &agg);
        assert!(md.contains('…'), "61-char question must be truncated with ellipsis");
        assert!(!md.contains(&q61), "full 61-char question must not appear verbatim");
    }

    /// invalid_entity_rate is computed as count/n, not count/1.
    #[test]
    fn test_invalid_entity_rate_denominator() {
        // 1 invalid_entity out of 4 tasks → rate = 0.25, not 1.0
        let results: Vec<QuestionResult> = (0..4)
            .map(|i| {
                let invalid = i == 0;
                make_result_full(
                    &format!("t{i}"),
                    false,
                    false,
                    invalid,
                    false,
                    0,
                    0,
                    100,
                    100,
                    500,
                    500,
                )
            })
            .collect();
        let agg = compute_aggregate(&results);
        assert_eq!(agg.arm_a_invalid_entity_count, 1);
        assert!((agg.arm_a_invalid_entity_rate - 0.25).abs() < 1e-9, "rate must be 1/4");
        // delta = b - a = 0 - 0.25 = -0.25
        assert!((agg.invalid_entity_delta - (-0.25)).abs() < 1e-9);
    }

    /// invalid_entity_rate: arm_b count used in arm_b_rate, not arm_a count.
    #[test]
    fn test_invalid_entity_rate_uses_correct_arm_count() {
        // arm_a: 0 invalid; arm_b: 3 invalid out of 3 tasks
        let results = vec![
            make_result_full("t1", true, true, false, true, 0, 0, 100, 100, 500, 500),
            make_result_full("t2", true, true, false, true, 0, 0, 100, 100, 500, 500),
            make_result_full("t3", true, true, false, true, 0, 0, 100, 100, 500, 500),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.arm_a_invalid_entity_count, 0);
        assert_eq!(agg.arm_b_invalid_entity_count, 3);
        assert!((agg.arm_a_invalid_entity_rate - 0.0).abs() < 1e-9);
        assert!((agg.arm_b_invalid_entity_rate - 1.0).abs() < 1e-9);
        // delta = b - a = 1.0 - 0.0 = 1.0 (positive = arm_a wins lower-is-better)
        assert!((agg.invalid_entity_delta - 1.0).abs() < 1e-9);
        assert_eq!(agg.invalid_entity_winner, "arm_a_sql");
    }

    /// Markdown renders `arms_equivalent` check/x correctly.
    #[test]
    fn test_markdown_equivalent_column() {
        let eq_result = QuestionResult {
            task_id: "t1".to_string(),
            question: "q".to_string(),
            arm_a: make_arm_full(Arm::SqlRunQuery, false, 0, 10, 100),
            arm_b: make_arm_full(Arm::MqoMultidimensional, false, 0, 10, 100),
            arms_equivalent: true,
            arm_a_pass: true,
            arm_b_pass: true,
            grader_reason: None,
        };
        let agg = compute_aggregate(&[eq_result.clone()]);
        let md = render_markdown(&[eq_result], &agg);
        // Should have ✓ for equivalent.
        assert!(md.contains('✓'));
    }
}
