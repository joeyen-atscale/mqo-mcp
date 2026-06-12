#![allow(clippy::doc_markdown, clippy::expect_used, clippy::unwrap_used, clippy::print_stderr, clippy::print_stdout)]
//! AC5: check_no_literal_pg_pass rejects --pg-pass literal with an error
//! that maps to exit 2 in the binary.

use mqo_session_footprint_meter::check_no_literal_pg_pass;

#[test]
fn ac5_rejects_literal_pg_pass_flag() {
    // A bare --pg-pass token.
    let cmd = "mqo-mcp-server --catalog tpcds --pg-pass secret123 --endpoint mcp-aws";
    assert!(
        check_no_literal_pg_pass(cmd).is_err(),
        "should reject --pg-pass literal"
    );
}

#[test]
fn ac5_rejects_literal_pg_pass_equals() {
    // --pg-pass=<value> form.
    let cmd = "mqo-mcp-server --catalog tpcds --pg-pass=secret123";
    assert!(
        check_no_literal_pg_pass(cmd).is_err(),
        "should reject --pg-pass=<value> form"
    );
}

#[test]
fn ac5_allows_pg_pass_env() {
    // The safe form: --pg-pass-env <VAR>.
    let cmd =
        "mqo-mcp-server --catalog tpcds --pg-pass-env ATSCALE_PG_PASS --endpoint mcp-aws --force-backend sql";
    assert!(
        check_no_literal_pg_pass(cmd).is_ok(),
        "should allow --pg-pass-env"
    );
}

#[test]
fn ac5_allows_no_creds() {
    let cmd = "mqo-mcp-server --catalog tpcds --endpoint mcp-aws";
    assert!(
        check_no_literal_pg_pass(cmd).is_ok(),
        "should allow command with no credentials flags"
    );
}

/// Verify the error message mentions the diagnostic so users know what to do.
#[test]
fn ac5_error_message_is_diagnostic() {
    let cmd = "mqo-mcp-server --pg-pass hunter2";
    let result = check_no_literal_pg_pass(cmd);
    assert!(result.is_err(), "should be an error");
    let msg = result.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(
        msg.contains("--pg-pass-env"),
        "error message should mention --pg-pass-env; got: {msg}"
    );
}
