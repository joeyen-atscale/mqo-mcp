//! Report computation and rendering (JSON + Markdown).

use crate::types::{AggregateReport, ErrorClass, QuestionResult};

// ── Helper structs for intermediate aggregation ─────────────────────────────

struct Errors {
    total_a: usize,
    total_b: usize,
    arith_a: usize,
    arith_b: usize,
    trans_a: usize,
    trans_b: usize,
    subset_a: usize,
    subset_b: usize,
}

struct Perf {
    retries_a: u64,
    retries_b: u64,
    latency_a: u64,
    latency_b: u64,
    tokens_a: u64,
    tokens_b: u64,
}

/// Count errors overall and by class across all results.
fn count_errors(results: &[QuestionResult]) -> Errors {
    Errors {
        total_a: results.iter().filter(|r| !r.arm_a_correct).count(),
        total_b: results.iter().filter(|r| !r.arm_b_correct).count(),
        arith_a: results
            .iter()
            .filter(|r| r.arm_a_verdict.error_class == ErrorClass::Arithmetic && !r.arm_a_correct)
            .count(),
        arith_b: results
            .iter()
            .filter(|r| r.arm_b_verdict.error_class == ErrorClass::Arithmetic && !r.arm_b_correct)
            .count(),
        trans_a: results
            .iter()
            .filter(|r| {
                r.arm_a_verdict.error_class == ErrorClass::Transcription && !r.arm_a_correct
            })
            .count(),
        trans_b: results
            .iter()
            .filter(|r| {
                r.arm_b_verdict.error_class == ErrorClass::Transcription && !r.arm_b_correct
            })
            .count(),
        subset_a: results
            .iter()
            .filter(|r| r.arm_a_verdict.error_class == ErrorClass::WrongSubset && !r.arm_a_correct)
            .count(),
        subset_b: results
            .iter()
            .filter(|r| r.arm_b_verdict.error_class == ErrorClass::WrongSubset && !r.arm_b_correct)
            .count(),
    }
}

/// Sum performance metrics across all results.
fn sum_perf(results: &[QuestionResult]) -> Perf {
    Perf {
        retries_a: results.iter().map(|r| u64::from(r.arm_a.n_retries)).sum(),
        retries_b: results.iter().map(|r| u64::from(r.arm_b.n_retries)).sum(),
        latency_a: results.iter().map(|r| r.arm_a.latency_ms).sum(),
        latency_b: results.iter().map(|r| r.arm_b.latency_ms).sum(),
        tokens_a: results.iter().map(|r| r.arm_a.tokens).sum(),
        tokens_b: results.iter().map(|r| r.arm_b.tokens).sum(),
    }
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Compute the aggregate report from per-question results.
#[must_use]
pub fn compute_aggregate(results: &[QuestionResult]) -> AggregateReport {
    let n = results.len();
    if n == 0 {
        return AggregateReport::zero();
    }

    let nf = n as f64;
    let ec = count_errors(results);
    let ps = sum_perf(results);

    // Value-error rates — lower is better.
    let arm_a_error_rate = ec.total_a as f64 / nf;
    let arm_b_error_rate = ec.total_b as f64 / nf;

    // Retries — lower is better.
    let arm_a_mean_retries = ps.retries_a as f64 / nf;
    let arm_b_mean_retries = ps.retries_b as f64 / nf;

    // Latency — lower is better.
    let arm_a_mean_latency_ms = ps.latency_a as f64 / nf;
    let arm_b_mean_latency_ms = ps.latency_b as f64 / nf;

    // Tokens — lower is better.
    let arm_a_mean_tokens = ps.tokens_a as f64 / nf;
    let arm_b_mean_tokens = ps.tokens_b as f64 / nf;

    // Signed class deltas stored as f64 to avoid any usize→isize cast at render time.
    let arithmetic_error_delta = ec.arith_b as f64 - ec.arith_a as f64;
    let transcription_error_delta = ec.trans_b as f64 - ec.trans_a as f64;
    let wrong_subset_error_delta = ec.subset_b as f64 - ec.subset_a as f64;

    AggregateReport {
        n_questions: n,
        arm_a_error_count: ec.total_a,
        arm_b_error_count: ec.total_b,
        arm_a_error_rate,
        arm_b_error_rate,
        error_rate_delta: arm_b_error_rate - arm_a_error_rate,
        error_rate_winner: lower_winner(arm_a_error_rate, arm_b_error_rate),
        arm_a_arithmetic_errors: ec.arith_a,
        arm_b_arithmetic_errors: ec.arith_b,
        arithmetic_error_delta,
        arm_a_transcription_errors: ec.trans_a,
        arm_b_transcription_errors: ec.trans_b,
        transcription_error_delta,
        arm_a_wrong_subset_errors: ec.subset_a,
        arm_b_wrong_subset_errors: ec.subset_b,
        wrong_subset_error_delta,
        arm_a_total_retries: ps.retries_a,
        arm_b_total_retries: ps.retries_b,
        arm_a_mean_retries,
        arm_b_mean_retries,
        retry_delta: arm_b_mean_retries - arm_a_mean_retries,
        retry_winner: lower_winner(arm_a_mean_retries, arm_b_mean_retries),
        arm_a_total_latency_ms: ps.latency_a,
        arm_b_total_latency_ms: ps.latency_b,
        arm_a_mean_latency_ms,
        arm_b_mean_latency_ms,
        latency_delta_ms: arm_b_mean_latency_ms - arm_a_mean_latency_ms,
        latency_winner: lower_winner(arm_a_mean_latency_ms, arm_b_mean_latency_ms),
        arm_a_total_tokens: ps.tokens_a,
        arm_b_total_tokens: ps.tokens_b,
        arm_a_mean_tokens,
        arm_b_mean_tokens,
        token_delta: arm_b_mean_tokens - arm_a_mean_tokens,
        token_winner: lower_winner(arm_a_mean_tokens, arm_b_mean_tokens),
    }
}

/// Return `"arm_a_raw_json"` / `"arm_b_handle"` / `"tie"` for a metric where lower = better.
fn lower_winner(a: f64, b: f64) -> String {
    if (b - a).abs() < 1e-9 {
        "tie".to_string()
    } else if b < a {
        "arm_b_handle".to_string()
    } else {
        "arm_a_raw_json".to_string()
    }
}

/// Render the full report as a Markdown string.
#[must_use]
pub fn render_markdown(results: &[QuestionResult], agg: &AggregateReport) -> String {
    let mut md = String::new();
    render_header(&mut md);
    render_aggregate_table(&mut md, agg);
    render_per_question_table(&mut md, results);
    md
}

fn render_header(md: &mut String) {
    md.push_str("# Handle vs Raw-JSON Benchmark Report\n\n");
    md.push_str("> **arm_a_raw_json**: model handed full rows, computes answer itself  \n");
    md.push_str(
        "> **arm_b_handle**: model gets summary+handle, server computes answer via `dataset_*` tools\n\n",
    );
}

fn render_aggregate_table(md: &mut String, agg: &AggregateReport) {
    md.push_str("## Aggregate Summary\n\n");
    md.push_str("| Metric | arm_a_raw_json | arm_b_handle | Delta (B−A) | Winner |\n");
    md.push_str("|--------|----------------|--------------|-------------|--------|\n");

    md.push_str(&format!(
        "| Value-error rate | {:.1}% ({}/{}) | {:.1}% ({}/{}) | {:+.1}% | **{}** |\n",
        agg.arm_a_error_rate * 100.0,
        agg.arm_a_error_count,
        agg.n_questions,
        agg.arm_b_error_rate * 100.0,
        agg.arm_b_error_count,
        agg.n_questions,
        agg.error_rate_delta * 100.0,
        agg.error_rate_winner
    ));
    md.push_str(&format!(
        "| — arithmetic errors | {} | {} | {:+.0} | |\n",
        agg.arm_a_arithmetic_errors, agg.arm_b_arithmetic_errors, agg.arithmetic_error_delta,
    ));
    md.push_str(&format!(
        "| — transcription errors | {} | {} | {:+.0} | |\n",
        agg.arm_a_transcription_errors,
        agg.arm_b_transcription_errors,
        agg.transcription_error_delta,
    ));
    md.push_str(&format!(
        "| — wrong-subset errors | {} | {} | {:+.0} | |\n",
        agg.arm_a_wrong_subset_errors, agg.arm_b_wrong_subset_errors, agg.wrong_subset_error_delta,
    ));
    md.push_str(&format!(
        "| Mean retries | {:.2} | {:.2} | {:+.2} | **{}** |\n",
        agg.arm_a_mean_retries, agg.arm_b_mean_retries, agg.retry_delta, agg.retry_winner
    ));
    md.push_str(&format!(
        "| Mean latency (ms) | {:.1} | {:.1} | {:+.1} | **{}** |\n",
        agg.arm_a_mean_latency_ms,
        agg.arm_b_mean_latency_ms,
        agg.latency_delta_ms,
        agg.latency_winner
    ));
    md.push_str(&format!(
        "| Mean tokens | {:.1} | {:.1} | {:+.1} | **{}** |\n",
        agg.arm_a_mean_tokens, agg.arm_b_mean_tokens, agg.token_delta, agg.token_winner
    ));

    md.push('\n');
}

fn render_per_question_table(md: &mut String, results: &[QuestionResult]) {
    md.push_str("## Per-Question Results\n\n");
    md.push_str(
        "| Task | Question | A correct | B correct | A error class | B error class | A retries | B retries | A latency ms | B latency ms |\n",
    );
    md.push_str(
        "|------|----------|-----------|-----------|---------------|---------------|-----------|-----------|-------------|-------------|\n",
    );

    for r in results {
        let q = r.question.chars().take(60).collect::<String>();
        let q = if r.question.len() > 60 {
            format!("{q}…")
        } else {
            q
        };
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.task_id,
            q,
            if r.arm_a_correct { "✓" } else { "✗" },
            if r.arm_b_correct { "✓" } else { "✗" },
            r.arm_a_verdict.error_class,
            r.arm_b_verdict.error_class,
            r.arm_a.n_retries,
            r.arm_b.n_retries,
            r.arm_a.latency_ms,
            r.arm_b.latency_ms,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Arm, ArmOutput, ErrorClass, GraderVerdict, QuestionResult};

    fn make_arm(arm: Arm, retries: u32, latency_ms: u64, tokens: u64) -> ArmOutput {
        ArmOutput {
            arm,
            payload_summary: "test".to_string(),
            reported_answer: serde_json::json!(42),
            error: None,
            n_retries: retries,
            latency_ms,
            tokens,
        }
    }

    fn make_verdict(correct: bool, class: ErrorClass) -> GraderVerdict {
        GraderVerdict {
            correct,
            error_class: class,
            reason: None,
        }
    }

    fn make_result(
        id: &str,
        a_correct: bool,
        b_correct: bool,
        a_class: ErrorClass,
        b_class: ErrorClass,
    ) -> QuestionResult {
        QuestionResult {
            task_id: id.to_string(),
            question: format!("Question {id}"),
            arm_a: make_arm(Arm::RawJson, 0, 300, 900),
            arm_b: make_arm(Arm::Handle, 0, 150, 700),
            arm_a_verdict: make_verdict(a_correct, a_class),
            arm_b_verdict: make_verdict(b_correct, b_class),
            arm_a_correct: a_correct,
            arm_b_correct: b_correct,
        }
    }

    #[test]
    fn test_compute_aggregate_empty() {
        let agg = compute_aggregate(&[]);
        assert_eq!(agg.n_questions, 0);
        assert_eq!(agg.error_rate_winner, "tie");
    }

    #[test]
    fn test_compute_aggregate_arm_b_wins_error_rate() {
        // arm_a errors on all; arm_b correct on all → arm_b wins (lower error rate)
        let results = vec![
            make_result("t1", false, true, ErrorClass::Arithmetic, ErrorClass::Correct),
            make_result("t2", false, true, ErrorClass::Transcription, ErrorClass::Correct),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.arm_a_error_rate, 1.0);
        assert_eq!(agg.arm_b_error_rate, 0.0);
        assert_eq!(agg.error_rate_winner, "arm_b_handle");
        assert_eq!(agg.arm_a_arithmetic_errors, 1);
        assert_eq!(agg.arm_a_transcription_errors, 1);
        assert_eq!(agg.arm_b_arithmetic_errors, 0);
    }

    #[test]
    fn test_compute_aggregate_all_correct() {
        let results = vec![
            make_result("t1", true, true, ErrorClass::Correct, ErrorClass::Correct),
            make_result("t2", true, true, ErrorClass::Correct, ErrorClass::Correct),
        ];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.arm_a_error_rate, 0.0);
        assert_eq!(agg.arm_b_error_rate, 0.0);
        assert_eq!(agg.error_rate_winner, "tie");
    }

    #[test]
    fn test_latency_winner_lower_is_better() {
        // arm_b has lower latency (150ms vs 300ms) → arm_b wins
        let results = vec![make_result(
            "t1",
            true,
            true,
            ErrorClass::Correct,
            ErrorClass::Correct,
        )];
        let agg = compute_aggregate(&results);
        assert_eq!(agg.latency_winner, "arm_b_handle");
    }

    #[test]
    fn test_render_markdown_contains_headers() {
        let results = vec![make_result(
            "t1",
            false,
            true,
            ErrorClass::Arithmetic,
            ErrorClass::Correct,
        )];
        let agg = compute_aggregate(&results);
        let md = render_markdown(&results, &agg);
        assert!(md.contains("# Handle vs Raw-JSON Benchmark Report"));
        assert!(md.contains("## Aggregate Summary"));
        assert!(md.contains("## Per-Question Results"));
        assert!(md.contains("arm_a_raw_json"));
        assert!(md.contains("arm_b_handle"));
        assert!(md.contains("Value-error rate"));
    }

    #[test]
    fn test_render_markdown_error_class_rows() {
        let results = vec![make_result(
            "t1",
            false,
            false,
            ErrorClass::WrongSubset,
            ErrorClass::Transcription,
        )];
        let agg = compute_aggregate(&results);
        let md = render_markdown(&results, &agg);
        assert!(md.contains("wrong_subset"));
        assert!(md.contains("transcription"));
    }
}
