//! Credential safety test: verify that `--pg-pass` is NOT a valid CLI argument.
//!
//! AC-critical: the CLI must reject a `--pg-pass` flag. Passwords must only be
//! supplied via an environment variable (using `--pg-pass-env`).

/// Verify `--pg-pass` is NOT in the CLI argument surface by inspecting clap's
/// help output — a simpler, deterministic check than trying to invoke the binary.
#[test]
fn pg_pass_flag_does_not_exist_in_arg_surface() {
    // Build the clap Command as used in main.rs and check its arguments.
    // We use the same Args struct via the lib.

    let help = build_help_text();

    // --pg-pass-env is allowed (env var name, not the password itself)
    assert!(
        help.contains("pg-pass-env"),
        "expected --pg-pass-env in help: {help}"
    );

    // --pg-pass (literal password) must NOT exist
    assert!(
        !help.contains("--pg-pass ") && !contains_pg_pass_flag(&help),
        "--pg-pass flag must not be present in CLI arg surface"
    );
}

fn build_help_text() -> String {
    // Use the public CLI schema — check the Args fields in the binary
    // by invoking the binary with --help and capturing output.
    //
    // We run the compiled test binary's sibling (the actual mqo-from-sql bin)
    // when available; otherwise fall back to static analysis of the help string.
    //
    // To avoid binary-not-built fragility in unit tests, we introspect clap
    // directly via a re-export from the lib.
    mqo_from_sql_lib::cli_help_text()
}

/// Returns true if the string contains a standalone `--pg-pass` flag
/// (i.e., not `--pg-pass-env`).
fn contains_pg_pass_flag(text: &str) -> bool {
    // Match --pg-pass that is NOT followed by "-env"
    text.contains("--pg-pass")
        && !text.lines().all(|line| {
            !line.contains("--pg-pass") || line.contains("--pg-pass-env")
        })
}
