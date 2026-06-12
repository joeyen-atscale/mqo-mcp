//! Benchmark runner: for each task, runs (or loads) both arm outputs,
//! grades with the external grader, and produces per-question results.

use crate::{
    grader,
    types::{Arm, ArmOutput, ErrorClass, GraderVerdict, QuestionResult, Task},
};
use std::collections::HashMap;
use thiserror::Error;

/// Error from the runner.
#[derive(Debug, Error)]
pub enum RunnerError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("grader error on task {task_id}: {source}")]
    Grader {
        task_id: String,
        #[source]
        source: grader::GraderError,
    },
}

/// Configuration for a benchmark run.
pub struct RunConfig {
    /// Path to the external grader binary or script.
    pub grader_cmd: String,

    /// If `Some`, load arm A outputs from this fixture file (no live cluster).
    pub fixture_a: Option<String>,

    /// If `Some`, load arm B outputs from this fixture file (no live cluster).
    pub fixture_b: Option<String>,
}

/// A fixture record: an [`ArmOutput`] tagged with `task_id`.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct FixtureRecord {
    pub task_id: String,
    #[serde(flatten)]
    pub output: ArmOutput,
}

/// Load a fixture file into a `HashMap<task_id, ArmOutput>`.
///
/// The file must be a JSON array of [`FixtureRecord`].
///
/// # Errors
///
/// Returns an error on I/O or JSON parse failure.
pub fn load_fixture_records(path: &str) -> Result<HashMap<String, ArmOutput>, RunnerError> {
    let contents = std::fs::read_to_string(path)?;
    let records: Vec<FixtureRecord> = serde_json::from_str(&contents)?;
    Ok(records.into_iter().map(|r| (r.task_id, r.output)).collect())
}

/// Run the benchmark over all tasks, using fixtures when configured.
///
/// For each task, both arm outputs are loaded (from fixture or as stub errors),
/// then the external grader is invoked for each arm to classify the reported
/// answer vs the known-correct answer.
///
/// # Errors
///
/// Returns [`RunnerError`] on fixture load, JSON parse, or grader invocation failures.
pub fn run(tasks: &[Task], config: &RunConfig) -> Result<Vec<QuestionResult>, RunnerError> {
    let fixture_a_map = config
        .fixture_a
        .as_deref()
        .map(load_fixture_records)
        .transpose()?
        .unwrap_or_default();

    let fixture_b_map = config
        .fixture_b
        .as_deref()
        .map(load_fixture_records)
        .transpose()?
        .unwrap_or_default();

    let mut results = Vec::with_capacity(tasks.len());

    for task in tasks {
        let arm_a = fixture_a_map.get(&task.id).cloned().unwrap_or_else(|| {
            stub_error_arm(
                Arm::RawJson,
                "no fixture and no live cluster configured",
            )
        });

        let arm_b = fixture_b_map.get(&task.id).cloned().unwrap_or_else(|| {
            stub_error_arm(
                Arm::Handle,
                "no fixture and no live cluster configured",
            )
        });

        // Grade arm A.
        let arm_a_verdict =
            grade_arm(&task.id, "a", &arm_a, &task.correct_answer, &config.grader_cmd)?;
        // Grade arm B.
        let arm_b_verdict =
            grade_arm(&task.id, "b", &arm_b, &task.correct_answer, &config.grader_cmd)?;

        let arm_a_correct = arm_a_verdict.correct;
        let arm_b_correct = arm_b_verdict.correct;

        results.push(QuestionResult {
            task_id: task.id.clone(),
            question: task.question.clone(),
            arm_a_correct,
            arm_b_correct,
            arm_a_verdict,
            arm_b_verdict,
            arm_a,
            arm_b,
        });
    }

    Ok(results)
}

/// Create a stub error [`ArmOutput`] for when no fixture or live cluster is available.
pub fn stub_error_arm(arm: Arm, msg: &str) -> ArmOutput {
    ArmOutput {
        arm,
        payload_summary: String::new(),
        reported_answer: serde_json::Value::Null,
        error: Some(msg.to_string()),
        n_retries: 0,
        latency_ms: 0,
        tokens: 0,
    }
}

/// Grade one arm's reported answer against the correct answer.
///
/// If the arm errored, returns a not-correct verdict without calling the grader.
fn grade_arm(
    task_id: &str,
    arm_label: &str,
    arm: &ArmOutput,
    correct_answer: &serde_json::Value,
    grader_cmd: &str,
) -> Result<GraderVerdict, RunnerError> {
    if arm.error.is_some() {
        return Ok(GraderVerdict {
            correct: false,
            error_class: ErrorClass::Arithmetic,
            reason: Some(format!(
                "arm_{arm_label} returned an error; skipping grader"
            )),
        });
    }

    grader::grade(grader_cmd, &arm.reported_answer, correct_answer).map_err(|source| {
        RunnerError::Grader {
            task_id: task_id.to_string(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Arm;

    #[test]
    fn test_stub_error_arm_has_error() {
        let arm = stub_error_arm(Arm::RawJson, "test error");
        assert!(arm.error.is_some());
        assert_eq!(arm.arm, Arm::RawJson);
        assert_eq!(arm.n_retries, 0);
        assert_eq!(arm.reported_answer, serde_json::Value::Null);
    }

    #[test]
    fn test_stub_error_arm_handle() {
        let arm = stub_error_arm(Arm::Handle, "handle err");
        assert_eq!(arm.arm, Arm::Handle);
        assert!(arm.error.is_some());
    }

    #[test]
    fn test_grade_arm_errored_skips_grader() {
        let arm = stub_error_arm(Arm::RawJson, "err");
        // grader_cmd is irrelevant because arm errored — no subprocess call.
        let v = grade_arm("t1", "a", &arm, &serde_json::json!(42), "/nonexistent/grader")
            .expect("errored arm must not call grader");
        assert!(!v.correct);
        assert!(v.reason.as_deref().unwrap_or("").contains("error"));
    }
}
