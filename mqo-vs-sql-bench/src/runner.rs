//! Benchmark runner: for each task, runs (or loads) both arm outputs,
//! grades with the external grader, and produces per-question results.

use crate::{
    grader,
    types::{Arm, ArmOutput, GraderVerdict, QuestionResult, Task},
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
        let arm_a = fixture_a_map
            .get(&task.id)
            .cloned()
            .unwrap_or_else(|| stub_error_arm(Arm::SqlRunQuery, "no fixture and no live cluster configured"));

        let arm_b = fixture_b_map
            .get(&task.id)
            .cloned()
            .unwrap_or_else(|| stub_error_arm(Arm::MqoMultidimensional, "no fixture and no live cluster configured"));

        let verdict = grade_arms(&task.id, &arm_a, &arm_b, &config.grader_cmd)?;

        // Pass: no error on that arm AND grader says equivalent.
        // When both arms return valid rows and the grader says equivalent, both pass.
        // When one arm errors and the other succeeds, the successful one is considered
        // to pass (grader verdict is set to the successful arm's result vs empty).
        let arm_a_errored = arm_a.error.is_some();
        let arm_b_errored = arm_b.error.is_some();

        let arm_a_pass = !arm_a_errored && verdict.equivalent;
        let arm_b_pass = !arm_b_errored && verdict.equivalent;

        results.push(QuestionResult {
            task_id: task.id.clone(),
            question: task.question.clone(),
            arm_a_pass,
            arm_b_pass,
            arms_equivalent: verdict.equivalent,
            grader_reason: verdict.reason,
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
        query: String::new(),
        rows: vec![],
        error: Some(msg.to_string()),
        n_retries: 0,
        latency_ms: 0,
        tokens: 0,
        invalid_entity: false,
    }
}

/// Grade the two arms by invoking the external grader.
///
/// If both arms errored, returns a not-equivalent verdict without calling the grader.
fn grade_arms(
    task_id: &str,
    arm_a: &ArmOutput,
    arm_b: &ArmOutput,
    grader_cmd: &str,
) -> Result<GraderVerdict, RunnerError> {
    if arm_a.error.is_some() && arm_b.error.is_some() {
        return Ok(GraderVerdict {
            equivalent: false,
            reason: Some("both arms returned errors".to_string()),
        });
    }

    grader::grade(grader_cmd, &arm_a.rows, &arm_b.rows).map_err(|source| RunnerError::Grader {
        task_id: task_id.to_string(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Arm;

    #[test]
    fn test_stub_error_arm_has_error() {
        let arm = stub_error_arm(Arm::SqlRunQuery, "test error");
        assert!(arm.error.is_some());
        assert_eq!(arm.arm, Arm::SqlRunQuery);
        assert_eq!(arm.n_retries, 0);
    }

    #[test]
    fn test_grade_arms_both_errored_no_grader_call() {
        let a = stub_error_arm(Arm::SqlRunQuery, "err");
        let b = stub_error_arm(Arm::MqoMultidimensional, "err");
        // grader_cmd is irrelevant because both arms errored — no subprocess call.
        let v = grade_arms("t1", &a, &b, "/nonexistent/grader").unwrap();
        assert!(!v.equivalent);
        assert!(v.reason.as_deref().unwrap_or("").contains("both arms"));
    }

    /// When only arm_a errors, arm_b does not → grader is called; arm_a fails, arm_b may pass.
    /// (Uses stub grader for real subprocess call.)
    #[test]
    fn test_run_arm_a_errors_arm_b_succeeds() {
        use std::path::Path;
        use crate::types::Task;
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let stub = format!("{manifest_dir}/fixtures/stub_grader.sh");
        // Only provide fixture for arm B; arm A will be stub-error.
        let fix_b = format!("{manifest_dir}/fixtures/arm_b_outputs.json");
        let tasks_path = format!("{manifest_dir}/fixtures/tasks.json");

        if !Path::new(&stub).exists() {
            return; // Skip in environments without fixture files.
        }

        let tasks_raw = std::fs::read_to_string(&tasks_path).unwrap();
        let tasks: Vec<Task> = serde_json::from_str(&tasks_raw).unwrap();
        let config = RunConfig {
            grader_cmd: stub,
            fixture_a: None,   // arm A will error
            fixture_b: Some(fix_b),
        };
        let results = run(&tasks, &config).unwrap();
        for r in &results {
            // arm_a errored → arm_a_pass must be false regardless of grader verdict.
            assert!(!r.arm_a_pass, "errored arm_a must not pass");
            // arm_b has no error → arm_b_pass follows grader verdict (stub says equivalent).
            // arm_b_pass = !arm_b_errored && verdict.equivalent = true && true = true
            assert!(r.arm_b_pass, "arm_b with no error and equivalent grader must pass");
        }
    }

    /// When only arm_b errors, arm_a succeeds → arm_b fails, arm_a may pass.
    #[test]
    fn test_run_arm_b_errors_arm_a_succeeds() {
        use std::path::Path;
        use crate::types::Task;
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let stub = format!("{manifest_dir}/fixtures/stub_grader.sh");
        let fix_a = format!("{manifest_dir}/fixtures/arm_a_outputs.json");
        let tasks_path = format!("{manifest_dir}/fixtures/tasks.json");

        if !Path::new(&stub).exists() {
            return;
        }

        let tasks_raw = std::fs::read_to_string(&tasks_path).unwrap();
        let tasks: Vec<Task> = serde_json::from_str(&tasks_raw).unwrap();
        let config = RunConfig {
            grader_cmd: stub,
            fixture_a: Some(fix_a),
            fixture_b: None,   // arm B will error
        };
        let results = run(&tasks, &config).unwrap();
        for r in &results {
            // arm_b errored → arm_b_pass must be false.
            assert!(!r.arm_b_pass, "errored arm_b must not pass");
            // arm_a has no error → arm_a_pass follows grader verdict.
            assert!(r.arm_a_pass, "arm_a with no error and equivalent grader must pass");
        }
    }

    /// Arm::Display produces the expected strings.
    #[test]
    fn test_arm_display() {
        assert_eq!(format!("{}", Arm::SqlRunQuery), "arm_a_sql");
        assert_eq!(format!("{}", Arm::MqoMultidimensional), "arm_b_mqo");
    }
}
