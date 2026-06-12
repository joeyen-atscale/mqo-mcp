//! Tests for the `render` Cargo feature.
//!
//! These tests are only compiled and run when the `render` feature is enabled:
//!   `cargo test --features render`
//!
//! All hermetic tests use `render_check_with_converter` / `corpus_render_gate_with_converter`
//! with a known-absent executable name — no unsafe env mutation needed.
//!
//! The integration test at the bottom is skipped automatically unless the
//! `vl-convert` CLI is present on PATH.

#![cfg(feature = "render")]
// These tests call `expect` / indexing on controlled data; allow it here.
#![allow(clippy::expect_used, clippy::indexing_slicing)]

use mqo_vega_emitter::emit;
use mqo_vega_emitter::render::{
    corpus_render_gate_with_converter, render_check_with_converter, RenderError, RenderFormat,
};
use serde_json::json;

/// A known-absent converter name used to trigger `ConverterNotFound` hermetically.
const ABSENT_CONVERTER: &str = "vl-convert-does-not-exist-in-any-path";

/// A minimal but structurally-valid Vega-Lite v5 spec (line chart).
fn good_spec_json() -> String {
    let rec = json!({
        "mark": "Line",
        "encoding": {
            "x": { "field": "year", "data_type": "temporal" },
            "y": { "field": "revenue", "data_type": "quantitative" }
        }
    });
    let rows = vec![
        json!({"year": "2023", "revenue": 100}),
        json!({"year": "2024", "revenue": 200}),
    ];
    let spec = emit(&rec, &rows).expect("emit must succeed for a valid recommendation");
    serde_json::to_string(&spec).expect("serialization must not fail")
}

// ---------------------------------------------------------------------------
// Test: emit rejects a malformed recommendation before the render stage.
// ---------------------------------------------------------------------------

#[test]
fn malformed_recommendation_is_rejected_by_emit() {
    let malformed = json!({
        // missing `mark` field
        "encoding": {
            "x": { "field": "year", "data_type": "temporal" }
        }
    });
    let rows = vec![json!({"year": "2023", "revenue": 100})];
    let result = emit(&malformed, &rows);
    assert!(
        result.is_err(),
        "emit should return Err for a recommendation missing `mark`"
    );
}

// ---------------------------------------------------------------------------
// Test: RenderFormat inferred from path extension.
// ---------------------------------------------------------------------------

#[test]
fn render_format_from_svg_path() {
    use std::path::Path;
    let fmt = RenderFormat::from_path(Path::new("out.svg"));
    assert_eq!(fmt, Some(RenderFormat::Svg));
}

#[test]
fn render_format_from_png_path() {
    use std::path::Path;
    let fmt = RenderFormat::from_path(Path::new("out.png"));
    assert_eq!(fmt, Some(RenderFormat::Png));
}

#[test]
fn render_format_unknown_extension_is_none() {
    use std::path::Path;
    let fmt = RenderFormat::from_path(Path::new("out.json"));
    assert!(fmt.is_none());
}

// ---------------------------------------------------------------------------
// Test: render_check returns ConverterNotFound when converter is absent.
//
// Uses a known-absent executable name — no unsafe env mutation.
// ---------------------------------------------------------------------------

#[test]
fn render_check_returns_converter_not_found_when_absent() {
    let spec = good_spec_json();
    let result =
        render_check_with_converter(&spec, "test-spec", RenderFormat::Svg, ABSENT_CONVERTER);

    assert!(
        matches!(result, Err(RenderError::ConverterNotFound)),
        "expected ConverterNotFound when converter is not on PATH, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// Test: corpus_render_gate names the failing spec.
// ---------------------------------------------------------------------------

#[test]
fn corpus_gate_names_first_failing_spec() {
    let spec1 = good_spec_json();
    let spec2 = good_spec_json();
    let corpus: Vec<(&str, &str)> = vec![
        ("spec-alpha", spec1.as_str()),
        ("spec-beta", spec2.as_str()),
    ];

    let result = corpus_render_gate_with_converter(&corpus, RenderFormat::Svg, ABSENT_CONVERTER);

    let (failing_id, _err) = result.expect_err("corpus gate must fail when converter is absent");
    assert_eq!(
        failing_id, "spec-alpha",
        "the first failing spec must be named in the error"
    );
}

// ---------------------------------------------------------------------------
// Integration test: render to SVG when vl-convert IS on PATH.
//
// Skipped automatically when vl-convert is not installed.
// Run manually: `pip install vl-convert-python && cargo test --features render`
// ---------------------------------------------------------------------------

#[test]
fn render_check_produces_nonempty_svg_when_vl_convert_present() {
    use mqo_vega_emitter::render::render_check;

    // Check whether vl-convert is available; skip if not.
    let available = std::process::Command::new("vl-convert")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !available {
        eprintln!("SKIP: vl-convert not on PATH — install with `pip install vl-convert-python`");
        return;
    }

    let spec = good_spec_json();
    let result = render_check(&spec, "integration-test-spec", RenderFormat::Svg);

    match result {
        Ok(bytes) => {
            assert!(!bytes.is_empty(), "rendered SVG must not be empty");
            let svg_str = String::from_utf8_lossy(&bytes);
            assert!(
                svg_str.contains("<svg") || svg_str.contains("<?xml"),
                "output should look like an SVG document"
            );
        }
        Err(e) => panic!("render_check failed unexpectedly: {e}"),
    }
}
