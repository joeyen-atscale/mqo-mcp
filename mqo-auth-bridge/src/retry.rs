//! Bounded retry logic for transient PGWire / engine errors (R1–R6).
//!
//! # Design
//!
//! Only **transient infrastructure-class** errors are retried — connection
//! resets, broken pipes, timeouts to connect, and generic "db error" responses
//! with no `SQLSTATE` code.  The predicate is **conservative**: when in doubt
//! the error is treated as non-retryable.
//!
//! `model_path` errors ([`EngineError::QueryError`]) are **never** retried —
//! retrying a wrong query wastes the deadline and can lower pass@1.
//!
//! # Backoff
//!
//! `base_ms * 2^attempt` with a small deterministic jitter derived from the
//! attempt index (no external RNG dependency).  The backoff is clamped to
//! `max_backoff_ms` per step and is also bounded by the remaining deadline
//! (R2): if the next wait + grace would exceed the deadline, the retry is
//! skipped and the current error is returned as-is (not as
//! `EngineErrorRetriedExhausted` — that only fires when retries are
//! *attempted* and all fail).

use std::time::{Duration, Instant};

use crate::error::EngineError;

// ─── Configuration ───────────────────────────────────────────────────────────

/// Default number of *extra* attempts after the first failure (total = 1 + max_retries).
pub const DEFAULT_ENGINE_MAX_RETRIES: u32 = 2;

/// Default base backoff duration per retry step (ms).
pub const DEFAULT_ENGINE_RETRY_BASE_MS: u64 = 100;

/// Default per-step backoff cap (ms).  Steps are clamped to this before adding
/// jitter so total worst-case added latency stays bounded.
pub const DEFAULT_ENGINE_RETRY_MAX_MS: u64 = 2_000;

/// Retry configuration for the engine execution loop.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of *retries* (0 disables retry; default 2 → 3 attempts
    /// total).
    pub max_retries: u32,
    /// Base sleep duration before the first retry (ms).
    pub base_ms: u64,
    /// Per-step sleep cap (ms); the exponential schedule is clamped to this.
    pub max_backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_ENGINE_MAX_RETRIES,
            base_ms: DEFAULT_ENGINE_RETRY_BASE_MS,
            max_backoff_ms: DEFAULT_ENGINE_RETRY_MAX_MS,
        }
    }
}

// ─── Transient predicate ──────────────────────────────────────────────────────

/// Returns `true` when `e` is a transient infrastructure error safe to retry.
///
/// Conservative: if the error does not match a known transient pattern it is
/// treated as non-retryable (R3 — `model_path` / deterministic errors must
/// never be retried).
///
/// Retryable:
/// - [`EngineError::ConnectionFailure`] — all connection-level errors
/// - [`EngineError::Postgres`] where the error message contains a transient
///   pattern ("connection reset", "broken pipe", "timeout", "db error" with no
///   SQLSTATE; see inline patterns for the exhaustive list)
///
/// Non-retryable:
/// - [`EngineError::QueryError`] — wrong query / model_path
/// - [`EngineError::AuthFailure`] / [`EngineError::MissingSecret`] — credential
///   problems are not fixed by waiting
/// - [`EngineError::QueryDeadlineExceeded`] — deadline already fired
/// - [`EngineError::RowCapTripped`] — result shape issue, not infrastructure
/// - [`EngineError::Http`] — non-timeout HTTP errors are treated as permanent
///   (could be auth 401, 403, etc.)
/// - [`EngineError::EngineErrorRetriedExhausted`] — already retried, do not
///   double-wrap
#[must_use]
pub fn is_transient(e: &EngineError) -> bool {
    match e {
        // Connection failures are always transient-retryable (R6 reconnect applies).
        EngineError::ConnectionFailure { .. } => true,

        // Postgres (PGWire) errors: inspect the message for known transient patterns.
        EngineError::Postgres(pg_err) => is_transient_postgres(pg_err),

        // Explicit non-retryable classes:
        EngineError::QueryError { .. }          // model_path — NEVER retry (R3)
        | EngineError::AuthFailure { .. }        // permanent credential failure
        | EngineError::MissingSecret { .. }      // startup misconfiguration
        | EngineError::Http(_)                   // HTTP errors (may be 401/403)
        | EngineError::RowCapTripped { .. }      // result shape / size issue
        | EngineError::QueryDeadlineExceeded { .. } // deadline already expired
        | EngineError::EngineErrorRetriedExhausted { .. } // already retried
        => false,
    }
}

/// Inspect a `tokio_postgres::Error` for known transient failure patterns.
///
/// The predicate is conservative: if no known-transient pattern matches,
/// returns `false` so we do not accidentally retry a deterministic server-side
/// rejection.
///
/// Patterns matched (case-insensitive substring):
/// - `"connection reset"` — TCP RST from the server
/// - `"broken pipe"` — write to a dead socket
/// - `"connection closed"` — PGWire closed mid-flight
/// - `"connection timed out"` — TCP connect timeout
/// - `"db error"` without a SQLSTATE — AtScale's generic transient "db error"
///
/// `SQLSTATE` present = server-side query rejection = model_path, non-retryable.
fn is_transient_postgres(e: &tokio_postgres::Error) -> bool {
    // A DbError with a code is a PostgreSQL server-side error (e.g. syntax,
    // relation-not-found, type mismatch).  These are model_path errors and must
    // NOT be retried.
    if e.as_db_error().is_some() {
        return false;
    }

    // No DbError → transport / IO / connection-level error.
    // Inspect the string representation for transient patterns.
    let msg = e.to_string().to_lowercase();
    msg.contains("connection reset")
        || msg.contains("broken pipe")
        || msg.contains("connection closed")
        || msg.contains("connection timed out")
        || msg.contains("db error")
        || msg.contains("io error")
        || msg.contains("eof")
        || msg.contains("unexpected eof")
}

// ─── Deterministic jitter ─────────────────────────────────────────────────────

/// Derive a small deterministic jitter (0..=base_ms/4) from the attempt index.
///
/// This avoids a `rand` dependency (not in mqo-auth-bridge's Cargo.toml) while
/// still spreading concurrent retries across a window.  For k=1 evals the
/// exact value does not matter; the formula is stable across Rust versions.
#[inline]
#[must_use]
fn jitter_ms(attempt: u32, base_ms: u64) -> u64 {
    // Simple deterministic spread: multiply attempt by a prime, take mod (base/4+1).
    let window = base_ms / 4 + 1;
    (u64::from(attempt).wrapping_mul(37).wrapping_add(13)) % window
}

// ─── Backoff computation ──────────────────────────────────────────────────────

/// Compute the sleep duration for a given retry `attempt` (0-based).
///
/// `base_ms * 2^attempt` clamped to `max_backoff_ms`, plus jitter.
/// Returns the duration as a [`Duration`].
#[must_use]
pub fn backoff_duration(attempt: u32, cfg: &RetryConfig) -> Duration {
    let shifted = cfg.base_ms.saturating_mul(1_u64 << attempt.min(30));
    let capped = shifted.min(cfg.max_backoff_ms);
    let with_jitter = capped + jitter_ms(attempt, cfg.base_ms);
    Duration::from_millis(with_jitter)
}

// ─── Retry loop ───────────────────────────────────────────────────────────────

/// Execute `f` with bounded retry on transient errors.
///
/// - `cfg.max_retries == 0` → single attempt, no retry (AC4).
/// - On a transient error: sleep `backoff_duration(attempt, cfg)`, then retry.
/// - On a non-transient error: return immediately (attempt == 1 for model_path,
///   etc.).
/// - When all retries are exhausted: return
///   [`EngineError::EngineErrorRetriedExhausted`] (R4).
/// - `deadline_end` bounds the total time: if the computed backoff would push
///   past the deadline, skip the sleep and return the last error directly (R2).
///   This prevents the retry loop from exceeding the per-query deadline.
///
/// `f` receives the attempt number (1-based) so it can reconnect on connection
/// failures (R6); the caller is responsible for reconnection when needed.
///
/// # Type parameter
///
/// `F: FnMut(u32) -> Result<T, EngineError>` — attempt number is 1-based.
pub fn with_retry<T, F>(mut f: F, cfg: &RetryConfig, deadline_end: Option<Instant>) -> Result<T, EngineError>
where
    F: FnMut(u32) -> Result<T, EngineError>,
{
    let max_attempts = cfg.max_retries.saturating_add(1); // retries=0 → 1 attempt
    let mut total_backoff_ms: u64 = 0;

    for attempt in 1..=max_attempts {
        match f(attempt) {
            Ok(v) => return Ok(v),
            Err(e) if attempt < max_attempts && is_transient(&e) => {
                // Transient error and we have retries left.
                let backoff = backoff_duration(attempt - 1, cfg); // 0-based backoff index

                // R2: respect deadline — if adding this backoff would exceed the
                // deadline, bail out now rather than sleeping past it.
                if let Some(end) = deadline_end {
                    let now = Instant::now();
                    if now + backoff >= end {
                        eprintln!(
                            "event=retry_deadline_cutoff attempt={attempt} \
                             backoff_ms={} remaining_ms={}; aborting retry to respect deadline",
                            backoff.as_millis(),
                            end.saturating_duration_since(now).as_millis(),
                        );
                        // Return the raw error (not exhausted — deadline wins, R2).
                        return Err(e);
                    }
                }

                let backoff_ms = backoff.as_millis() as u64;
                total_backoff_ms = total_backoff_ms.saturating_add(backoff_ms);
                eprintln!(
                    "event=engine_retry attempt={attempt}/{max_attempts} \
                     backoff_ms={backoff_ms} error=\"{}\"",
                    e
                );
                std::thread::sleep(backoff);
            }
            Err(e) if attempt < max_attempts => {
                // Non-transient error — return immediately without retry (R3).
                return Err(e);
            }
            Err(e) => {
                // Final attempt failed (could be transient or non-transient on
                // the last attempt).
                if is_transient(&e) && max_attempts > 1 {
                    // We attempted retries and they all failed.
                    eprintln!(
                        "event=engine_retry_exhausted attempts={max_attempts} \
                         total_backoff_ms={total_backoff_ms} final_error=\"{}\"",
                        e
                    );
                    return Err(EngineError::EngineErrorRetriedExhausted {
                        attempts: max_attempts,
                        total_backoff_ms,
                        message: e.to_string(),
                    });
                }
                // Non-transient on last attempt (or single-attempt path): return as-is.
                return Err(e);
            }
        }
    }

    // Unreachable: the loop always returns inside (Ok or Err branch).
    unreachable!("retry loop exhausted without returning")
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helper: build a RetryConfig with 2 retries and tiny backoffs ──────────

    fn fast_cfg() -> RetryConfig {
        RetryConfig {
            max_retries: 2,
            base_ms: 1,
            max_backoff_ms: 10,
        }
    }

    fn no_retry_cfg() -> RetryConfig {
        RetryConfig {
            max_retries: 0,
            base_ms: 1,
            max_backoff_ms: 10,
        }
    }

    // ── Transient predicate ───────────────────────────────────────────────────

    #[test]
    fn connection_failure_is_transient() {
        let e = EngineError::ConnectionFailure {
            reason: "connection refused".to_string(),
        };
        assert!(is_transient(&e), "ConnectionFailure must be transient-retryable");
    }

    #[test]
    fn query_error_is_never_retryable() {
        let e = EngineError::QueryError {
            reason: "column does not exist".to_string(),
        };
        assert!(
            !is_transient(&e),
            "QueryError (model_path) must NEVER be retryable (R3)"
        );
    }

    #[test]
    fn auth_failure_is_not_retryable() {
        let e = EngineError::AuthFailure {
            reason: "401 Unauthorized".to_string(),
        };
        assert!(!is_transient(&e), "AuthFailure must not be retryable");
    }

    #[test]
    fn deadline_exceeded_is_not_retryable() {
        let e = EngineError::QueryDeadlineExceeded {
            elapsed_secs: 60,
            deadline_secs: 60,
            hint: "retry a cheaper shape".to_string(),
        };
        assert!(!is_transient(&e), "QueryDeadlineExceeded must not be retryable");
    }

    #[test]
    fn retried_exhausted_is_not_retryable() {
        let e = EngineError::EngineErrorRetriedExhausted {
            attempts: 3,
            total_backoff_ms: 300,
            message: "db error".to_string(),
        };
        assert!(!is_transient(&e), "EngineErrorRetriedExhausted must not double-wrap");
    }

    // ── Retry loop: retry-then-success ────────────────────────────────────────

    #[test]
    fn transient_failure_then_success_returns_ok() {
        // Attempt 1 → transient error; attempt 2 → success.
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let cc = call_count.clone();

        let result = with_retry(
            |_attempt| {
                let n = cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if n == 1 {
                    Err(EngineError::ConnectionFailure {
                        reason: "connection reset by peer".to_string(),
                    })
                } else {
                    Ok(42_u32)
                }
            },
            &fast_cfg(),
            None,
        );

        assert_eq!(result.unwrap(), 42, "should succeed on attempt 2");
        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "should have called f exactly twice"
        );
    }

    // ── Retry loop: exhaustion → EngineErrorRetriedExhausted ─────────────────

    #[test]
    fn exhaustion_returns_retried_exhausted() {
        let cfg = RetryConfig { max_retries: 2, base_ms: 1, max_backoff_ms: 5 };
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let cc = call_count.clone();

        let result: Result<u32, EngineError> = with_retry(
            |_| {
                cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(EngineError::ConnectionFailure {
                    reason: "broken pipe".to_string(),
                })
            },
            &cfg,
            None,
        );

        match result {
            Err(EngineError::EngineErrorRetriedExhausted { attempts, total_backoff_ms, .. }) => {
                assert_eq!(attempts, 3, "max_retries=2 → 3 total attempts");
                assert!(total_backoff_ms > 0, "should have accumulated backoff");
            }
            other => panic!("expected EngineErrorRetriedExhausted, got {other:?}"),
        }
        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "should have attempted 3 times"
        );
    }

    // ── Retry loop: max_retries=0 disables retry ──────────────────────────────

    #[test]
    fn max_retries_zero_disables_retry() {
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let cc = call_count.clone();

        let result: Result<u32, EngineError> = with_retry(
            |_| {
                cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(EngineError::ConnectionFailure {
                    reason: "connection reset".to_string(),
                })
            },
            &no_retry_cfg(),
            None,
        );

        // Should be a raw ConnectionFailure (not EngineErrorRetriedExhausted)
        // because retries are disabled.
        assert!(
            matches!(result, Err(EngineError::ConnectionFailure { .. })),
            "max_retries=0 should return the raw error on one attempt"
        );
        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "max_retries=0 → exactly one attempt"
        );
    }

    // ── Retry loop: model_path error is NEVER retried ─────────────────────────

    #[test]
    fn model_path_error_never_retried() {
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let cc = call_count.clone();

        let result: Result<u32, EngineError> = with_retry(
            |_| {
                cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(EngineError::QueryError {
                    reason: "column \"nonexistent\" does not exist".to_string(),
                })
            },
            &fast_cfg(), // max_retries=2, but should still only make 1 attempt
            None,
        );

        assert!(
            matches!(result, Err(EngineError::QueryError { .. })),
            "model_path QueryError must be returned immediately without retry"
        );
        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "QueryError (model_path) must only result in 1 attempt — never retried (R3)"
        );
    }

    // ── Retry loop: deadline cutoff ───────────────────────────────────────────

    #[test]
    fn deadline_cutoff_stops_retry() {
        // Set a deadline that has already passed — the retry should bail out
        // immediately after the first failure.
        let already_expired = Instant::now() - Duration::from_secs(1);
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let cc = call_count.clone();

        let result: Result<u32, EngineError> = with_retry(
            |_| {
                cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(EngineError::ConnectionFailure {
                    reason: "connection reset".to_string(),
                })
            },
            &fast_cfg(),
            Some(already_expired),
        );

        // Should return the raw error (deadline wins — not EngineErrorRetriedExhausted).
        assert!(
            matches!(result, Err(EngineError::ConnectionFailure { .. })),
            "deadline cutoff should return the raw error, not EngineErrorRetriedExhausted"
        );
        assert_eq!(
            call_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "deadline cutoff after first attempt → only 1 call"
        );
    }

    // ── Backoff duration is deterministic ─────────────────────────────────────

    #[test]
    fn backoff_increases_with_attempts() {
        let cfg = RetryConfig { max_retries: 3, base_ms: 100, max_backoff_ms: 10_000 };
        let b0 = backoff_duration(0, &cfg);
        let b1 = backoff_duration(1, &cfg);
        let b2 = backoff_duration(2, &cfg);
        // Each step should be >= the previous (exponential growth).
        assert!(b1 >= b0, "backoff should not decrease: b1={b1:?} b0={b0:?}");
        assert!(b2 >= b1, "backoff should not decrease: b2={b2:?} b1={b1:?}");
    }
}
