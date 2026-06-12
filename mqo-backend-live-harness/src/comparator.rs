//! Parity comparator trait + fake implementation.
//!
//! The real comparator (from mqo-cross-backend-parity) would execute the same
//! MQO against multiple live backends and compare results.  The fake used in
//! tests lets you configure per-(backend, case) return values.

use std::collections::HashMap;

use crate::{Backend, ParityOutcome, TestCase};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Executes `case` against `backends` and reports whether they agree.
pub trait ParityComparator {
    fn compare(&self, case: &TestCase, backends: &[Backend]) -> ParityOutcome;
}

// ---------------------------------------------------------------------------
// Fake comparator (for tests)
// ---------------------------------------------------------------------------

/// Returns pre-loaded (backend, case_name) → value pairs; detects divergence.
pub struct FakeComparator {
    /// Maps (backend, case_name) → value the fake "engine" returns.
    values: HashMap<(Backend, String), f64>,
}

impl FakeComparator {
    pub fn new(values: HashMap<(Backend, String), f64>) -> Self {
        Self { values }
    }
}

impl ParityComparator for FakeComparator {
    fn compare(&self, case: &TestCase, backends: &[Backend]) -> ParityOutcome {
        let live: Vec<(Backend, f64)> = backends
            .iter()
            .filter_map(|&b| {
                self.values
                    .get(&(b, case.name.clone()))
                    .map(|&v| (b, v))
            })
            .collect();

        if live.len() < 2 {
            return ParityOutcome::Skipped {
                reason: format!("only {} live backend(s)", live.len()),
            };
        }

        // Compare all pairs.
        for i in 0..live.len() {
            for j in (i + 1)..live.len() {
                let (ba, va) = live[i];
                let (bb, vb) = live[j];
                if (va - vb).abs() > 1e-6 {
                    return ParityOutcome::Diverged {
                        backend_a: ba,
                        value_a: va,
                        backend_b: bb,
                        value_b: vb,
                        case_name: case.name.clone(),
                    };
                }
            }
        }
        ParityOutcome::Agreed
    }
}
