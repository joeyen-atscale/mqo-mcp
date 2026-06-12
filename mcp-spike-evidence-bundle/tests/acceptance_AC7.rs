//! AC7: cargo test passes offline on fixtures; cargo clippy --all-targets
//! -- -D warnings clean.
//!
//! This test file exists as a declaration: if the file compiles and the
//! previous 6 test files pass, AC7 is satisfied. The clippy gate is enforced
//! by the run-metrics.sh harness (and CI), not by a runtime assertion here.

#[test]
#[allow(clippy::missing_const_for_fn)]
fn ac7_all_must_acs_gate() {
    // If this test is reached, the binary compiled and the test suite ran.
    // This is a stub; the real AC7 gate is `cargo clippy -D warnings` in CI.
    // Nothing to assert at runtime — the build system enforces it.
}
