//! External grader invocation.
//!
//! The grader is a configurable shell command. We write the reported answer
//! and the correct answer to a temp file as a JSON object:
//! `{ "reported": <reported_answer>, "correct": <correct_answer> }`
//! then invoke the grader with that file path as the sole argument.
//! The grader must emit a [`GraderVerdict`] JSON object on stdout.

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

/// Invoke the external grader to check whether the reported answer matches
/// the correct answer.
///
/// The grader binary is called as:
/// ```text
/// <grader_cmd> <input_json_file>
/// ```
/// where the input file contains:
/// ```json
/// { "reported": <reported_answer>, "correct": <correct_answer> }
/// ```
/// The grader must emit a [`GraderVerdict`] JSON object on stdout.
///
/// # Errors
///
/// Returns [`GraderError`] on I/O failure, non-zero grader exit, or JSON parse failure.
pub fn grade(
    grader_cmd: &str,
    reported: &Value,
    correct: &Value,
) -> Result<GraderVerdict, GraderError> {
    // Write input to a temp file.
    let input = serde_json::json!({ "reported": reported, "correct": correct });
    let input_str = serde_json::to_string(&input)?;

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

/// Generate a unique temp file path using PID + thread-id + nanos to avoid
/// collisions when multiple tests run in parallel.
fn tempfile_path() -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let pid = std::process::id();
    let tid = {
        // Use the lower 64 bits of the thread-id debug representation as a
        // cheap unique-per-thread discriminator (stable on all Tier-1 targets).
        let raw = format!("{:?}", std::thread::current().id());
        // ThreadId debug looks like "ThreadId(N)" — extract N, fall back to 0.
        raw.trim_start_matches("ThreadId(")
            .trim_end_matches(')')
            .parse::<u64>()
            .unwrap_or(0)
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("dh_bench_grader_{pid}_{tid}_{nanos}.json"))
}
