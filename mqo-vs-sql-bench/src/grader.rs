//! External grader invocation.
//!
//! The grader is a configurable shell command.  We write the two result sets
//! to a temp file as a JSON object `{ "a": [...], "b": [...] }`, invoke the
//! grader with that file path as the sole argument, and parse its stdout as a
//! [`GraderVerdict`].

use crate::types::GraderVerdict;
use serde_json::Value;
use std::process::Command;
use thiserror::Error;

/// Error from the grader invocation.
#[derive(Debug, Error)]
pub enum GraderError {
    #[error("grader I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("grader exited with status {status}: {stderr}")]
    NonZeroExit { status: i32, stderr: String },

    #[error("grader output is not valid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
}

/// Invoke the external grader to compare two result sets.
///
/// The grader binary is called as:
/// ```text
/// <grader_cmd> <input_json_file>
/// ```
/// where the input file contains `{ "a": <rows_a>, "b": <rows_b> }`.
/// The grader must emit a [`GraderVerdict`] JSON object on stdout.
///
/// # Errors
///
/// Returns [`GraderError`] on I/O failure, non-zero grader exit, or JSON parse failure.
pub fn grade(
    grader_cmd: &str,
    rows_a: &[Value],
    rows_b: &[Value],
) -> Result<GraderVerdict, GraderError> {
    // Write input to a temp file.
    let input = serde_json::json!({ "a": rows_a, "b": rows_b });
    let input_str = serde_json::to_string(&input)?;

    // Use a temp file so the grader can be any executable that reads a path.
    let tmp = tempfile_path();
    std::fs::write(&tmp, &input_str)?;

    // Shell out.
    let output = Command::new(grader_cmd).arg(&tmp).output()?;

    // Clean up temp file (best-effort).
    let _ = std::fs::remove_file(&tmp);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let code = output.status.code().unwrap_or(-1);
        return Err(GraderError::NonZeroExit {
            status: code,
            stderr,
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let verdict: GraderVerdict = serde_json::from_str(stdout.trim())?;
    Ok(verdict)
}

/// Generate a unique temp file path (no tempfile crate dependency at runtime).
fn tempfile_path() -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    std::env::temp_dir().join(format!("mqo_bench_grader_{ts}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grader that exits 0 with valid JSON is accepted.
    #[test]
    fn test_grade_success_with_valid_grader() {
        // Use the stub grader if available.
        let manifest = env!("CARGO_MANIFEST_DIR");
        let stub = format!("{manifest}/fixtures/stub_grader.sh");
        if !std::path::Path::new(&stub).exists() {
            return; // Skip in environments without fixtures.
        }
        let rows: Vec<Value> = vec![];
        let verdict = grade(&stub, &rows, &rows).expect("stub grader must succeed");
        assert!(verdict.equivalent, "stub grader always says equivalent");
    }

    /// A grader that exits non-zero returns `NonZeroExit` error.
    #[test]
    fn test_grade_nonzero_exit_returns_error() {
        // `false` is a shell built-in that always exits 1.
        let rows: Vec<Value> = vec![];
        // Try `false` as grader — it exits 1 and emits nothing.
        let result = grade("false", &rows, &rows);
        match result {
            Err(GraderError::NonZeroExit { status, .. }) => {
                assert!(status != 0, "NonZeroExit status must be non-zero; got {status}");
            }
            Err(GraderError::Io(_)) => {
                // `false` not on PATH or not executable — acceptable in CI.
            }
            Ok(_) => panic!("grader exiting 1 must not succeed"),
            Err(e) => panic!("unexpected error variant: {e}"),
        }
    }

    /// A grader that emits invalid JSON returns `InvalidJson` error.
    #[test]
    fn test_grade_invalid_json_returns_error() {
        // `echo` emits the argument as a string — not valid GraderVerdict JSON.
        let rows: Vec<Value> = vec![];
        // Use a shell script that emits garbage JSON.
        let garbage_grader = format!(
            "{}/fixtures/garbage_grader.sh",
            env!("CARGO_MANIFEST_DIR")
        );
        // Create a temp garbage grader if the canonical path doesn't exist.
        let grader_path = if std::path::Path::new(&garbage_grader).exists() {
            garbage_grader
        } else {
            // Write a one-shot grader that emits invalid JSON.
            let tmp = std::env::temp_dir().join("mqo_test_garbage_grader.sh");
            std::fs::write(&tmp, b"#!/bin/sh\necho 'not valid json'\n").ok();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&tmp).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&tmp, perms).ok();
            }
            tmp.to_string_lossy().into_owned()
        };

        let result = grade(&grader_path, &rows, &rows);
        match result {
            Err(GraderError::InvalidJson(_)) => {} // expected
            Err(GraderError::Io(_)) => {}          // acceptable: script not executable in some envs
            Ok(_) => panic!("garbage JSON grader must not succeed"),
            Err(e) => panic!("unexpected error variant: {e}"),
        }
    }
}
