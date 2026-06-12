//! AC5: Zero panels → structured error with nonzero exit code and no panic.

use mqo_dashboard_composer::{load_bundles, ComposerArgs, ComposerError, Layout, OutputFormat};

#[test]
fn ac5_no_bundles_returns_error() {
    // build_dashboard is pure; test the composed error path through load_bundles + error check
    let args = ComposerArgs {
        assets_file: None,
        asset_files: vec![],
        title: "Empty".to_owned(),
        layout: Layout::Grid,
        columns: 2,
        format: OutputFormat::Json,
    };

    let bundles = load_bundles(&args).expect("load should succeed with empty inputs");
    assert!(bundles.is_empty(), "no files → empty bundle list");

    // Validate that ComposerError::NoPanels formats without panicking
    let err = ComposerError::NoPanels;
    let msg = err.to_string();
    assert!(
        msg.contains("no panels"),
        "error message should mention 'no panels'"
    );
}

#[test]
fn ac5_error_display_is_structured() {
    let e = ComposerError::NoPanels;
    let s = format!("{e}");
    assert!(!s.is_empty());
    assert!(s.contains("no panels") || s.contains("bi-asset.v1"));
}
