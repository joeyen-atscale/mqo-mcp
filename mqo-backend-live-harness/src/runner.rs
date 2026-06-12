//! Case runner: probe → compile → execute → assert.
//!
//! The runner is parameterised over:
//! - A [`CapabilityProbe`] that reports backend liveness
//! - An [`Engine`] that compiles+executes an MQO and returns a scalar value
//! - A [`ParityComparator`] that cross-checks live backends

use std::collections::HashMap;

use crate::{
    comparator::ParityComparator,
    probe::CapabilityProbe,
    Backend, BackendStatus, CaseResult, CheckOutcome, HarnessReport, ParityOutcome, TestCase,
};

// ---------------------------------------------------------------------------
// Engine trait
// ---------------------------------------------------------------------------

/// Compiles and executes a single test case against one backend.
///
/// Returns `Ok(value)` on success, `Err(reason)` on any failure.
pub trait Engine {
    fn execute(&self, backend: Backend, case: &TestCase) -> Result<f64, String>;
}

// ---------------------------------------------------------------------------
// Fake engine (for tests)
// ---------------------------------------------------------------------------

/// Returns pre-loaded values; returns Err for any (backend, case) not in the map.
pub struct FakeEngine {
    /// Maps (backend, case_name) → value the fake "engine" returns.
    pub values: HashMap<(Backend, String), f64>,
}

impl FakeEngine {
    pub fn new(values: HashMap<(Backend, String), f64>) -> Self {
        Self { values }
    }
}

impl Engine for FakeEngine {
    fn execute(&self, backend: Backend, case: &TestCase) -> Result<f64, String> {
        self.values
            .get(&(backend, case.name.clone()))
            .copied()
            .ok_or_else(|| format!("FakeEngine: no value for ({backend}, {})", case.name))
    }
}

// ---------------------------------------------------------------------------
// run_harness
// ---------------------------------------------------------------------------

/// Run all (backend × case) checks, then run parity if ≥2 backends are live.
pub fn run_harness(
    backends: &[Backend],
    cases: &[TestCase],
    probe: &dyn CapabilityProbe,
    engine: &dyn Engine,
    comparator: &dyn ParityComparator,
) -> HarnessReport {
    // Step 1: probe all backends once.
    let statuses: HashMap<Backend, BackendStatus> = backends
        .iter()
        .map(|&b| (b, probe.probe(b)))
        .collect();

    // Step 2: for each (backend, case), decide pass/skip/fail.
    let mut results: Vec<CaseResult> = Vec::new();
    for &backend in backends {
        let status = statuses.get(&backend).unwrap();
        for case in cases {
            let outcome = if let Some(reason) = status.skip_reason() {
                CheckOutcome::Skip { reason }
            } else {
                match case.expected_value {
                    None => CheckOutcome::Skip {
                        reason: "parity-only (no expected_value)".to_string(),
                    },
                    Some(expected) => match engine.execute(backend, case) {
                        Err(reason) => CheckOutcome::Fail { reason },
                        Ok(got) => {
                            let diff = (got - expected).abs();
                            // Allow up to 0.01% relative tolerance.
                            let tol = expected.abs() * 1e-4 + 1e-6;
                            if diff <= tol {
                                CheckOutcome::Pass
                            } else {
                                CheckOutcome::Fail {
                                    reason: format!(
                                        "expected {:.6} got {:.6} (diff {:.6})",
                                        expected, got, diff
                                    ),
                                }
                            }
                        }
                    },
                }
            };
            results.push(CaseResult {
                backend,
                case_name: case.name.clone(),
                outcome,
            });
        }
    }

    // Step 3: parity check across all live backends (per case).
    let live_backends: Vec<Backend> = backends
        .iter()
        .filter(|&&b| statuses.get(&b).is_some_and(|s| s.is_live()))
        .copied()
        .collect();

    let mut parity: Vec<ParityOutcome> = Vec::new();
    if live_backends.len() >= 2 {
        for case in cases {
            parity.push(comparator.compare(case, &live_backends));
        }
    } else {
        parity.push(ParityOutcome::Skipped {
            reason: format!("only {} live backend(s)", live_backends.len()),
        });
    }

    HarnessReport { results, parity }
}
