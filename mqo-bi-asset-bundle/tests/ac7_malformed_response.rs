//! AC7 — a malformed response (no rows / no bound) returns a structured error,
//! not a panic, with a nonzero exit code (tested via the library error return).

use mqo_bi_asset_bundle::build_asset;
use serde_json::json;

#[test]
fn ac7_missing_rows_returns_error() {
    let response = json!({"bound": {"measures": ["revenue"], "dimensions": []}});
    let catalog = json!({"columns": [
        {"unique_name": "revenue", "label": "Revenue", "kind": "measure"}
    ]});
    let result = build_asset(&response, &catalog);
    assert!(
        result.is_err(),
        "build_asset must return Err when 'rows' is absent, got Ok"
    );
    let err_str = result.unwrap_err().to_string();
    assert!(
        !err_str.is_empty(),
        "error message must be non-empty"
    );
}

#[test]
fn ac7_missing_bound_returns_error() {
    let response = json!({"rows": [{"revenue": 100.0}]});
    let catalog = json!({"columns": [
        {"unique_name": "revenue", "label": "Revenue", "kind": "measure"}
    ]});
    let result = build_asset(&response, &catalog);
    assert!(
        result.is_err(),
        "build_asset must return Err when 'bound' is absent, got Ok"
    );
}

#[test]
fn ac7_completely_empty_object_returns_error() {
    let response = json!({});
    let catalog = json!({"columns": []});
    let result = build_asset(&response, &catalog);
    assert!(
        result.is_err(),
        "build_asset must return Err for an empty response object"
    );
}

#[test]
fn ac7_error_is_structured_not_panic() {
    // This test verifies the library returns Result::Err (structured) rather than
    // panicking. The test harness catches panics, so if this passes the library
    // doesn't panic — the assert is belt-and-suspenders.
    let result = std::panic::catch_unwind(|| {
        let response = json!({"bad": "payload"});
        let catalog = json!({"columns": []});
        build_asset(&response, &catalog)
    });
    assert!(
        result.is_ok(),
        "build_asset must not panic on a malformed response"
    );
    let inner = result.unwrap();
    assert!(
        inner.is_err(),
        "build_asset must return Err (not Ok) for a malformed response"
    );
}
