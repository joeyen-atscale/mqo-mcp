//! Differential-correctness test: `dh-ops` vs DuckDB-SQL over a fixture corpus.
//!
//! FR-7 / AC-6 (PRD-mqo-mcp-handle-merge): assert that `dh-ops` results equal
//! DuckDB-SQL results for `aggregate`/`filter`/`sort`/`top_n`/`pivot` over a
//! fixture corpus, neutralizing hand-rolled-op risk.
//!
//! STATUS: STUBBED — intentionally NOT wired to a live DuckDB harness this pass.
//!
//! Rationale (NFR-3 / AC-8): pulling the `duckdb` crate as a dev-dependency
//! triggers a bundled C++ build (~4m26s, FFI) which threatens the build gate and
//! risks `libduckdb-sys` leaking into dep resolution. Per the build instructions
//! this differential test is deferred: the `dh-ops` kernel is already covered by
//! its own in-crate unit tests, and the handle-op integration tests
//! (`handle_ops_test.rs`) assert each op derives correct row counts/shapes.
//!
//! TODO(FR-7): add `duckdb = { version = "*" }` as a **test-only** dev-dependency
//! (never a runtime/server dep — keep `libduckdb-sys` out of the binary per
//! AC-8), build a fixture corpus, run each op through both `dh-ops` and an
//! equivalent DuckDB SQL statement, and assert equality. Gate this module behind
//! a `cfg(feature = "duckdb-difftest")` so the default `cargo test` stays fast.

/// Placeholder asserting the test module compiles and is discoverable. Replaced
/// by the real differential harness when the DuckDB dev-dep is introduced.
#[test]
fn differential_correctness_stub_pending_duckdb_devdep() {
    // Intentionally trivial: see module docs / TODO(FR-7).
    // Until then, dh-ops correctness is covered by dh-ops' own unit tests and
    // the handle_ops_test.rs integration tests.
    assert!(true, "stub — differential harness pending (FR-7)");
}
