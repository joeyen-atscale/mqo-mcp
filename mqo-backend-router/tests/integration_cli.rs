//! Integration tests for the `mqo-route` binary (AC7).
//!
//! These tests build the binary and invoke it via `std::process::Command`,
//! exercising all three routing paths end-to-end with real JSON fixture files
//! written to a temp directory. This catches missed mutants in dispatch arms
//! that `cargo test --lib` cannot reach.
//!
//! Per SKILL.md §"Binary integration test gap (mqo-spec, 2026-06-08)":
//! add `tests/integration_cli.rs` for every `[[bin]]` project.

use std::fs;
use std::process::Command;
use tempfile::TempDir;

// ── Fixture constructors ─────────────────────────────────────────────────────

/// Minimal BoundMqo JSON with one measure and zero dimensions.
#[allow(dead_code)]
fn bound_mqo_scalar(model: &str) -> String {
    serde_json::json!({
        "mqo": {
            "model": model,
            "measures": [{"unique_name": "sales.revenue"}],
            "dimensions": [],
            "filters": [],
            "time_intelligence": [],
            "order": null,
            "limit": null,
            "non_empty": false
        },
        "measures": [{"unique_name": "sales.revenue", "is_calc": false, "semi_additive": false, "required_dimension": null}],
        "dimensions": []
    })
    .to_string()
}

/// BoundMqo JSON with two low-cardinality dimension levels.
fn bound_mqo_low_card() -> String {
    serde_json::json!({
        "mqo": {
            "model": "sales",
            "measures": [{"unique_name": "sales.revenue"}],
            "dimensions": [
                {"hierarchy": "time.calendar", "level": "Year"},
                {"hierarchy": "geo.country", "level": "Country"}
            ],
            "filters": [],
            "time_intelligence": [],
            "order": null,
            "limit": null,
            "non_empty": false
        },
        "measures": [{"unique_name": "sales.revenue", "is_calc": false, "semi_additive": false, "required_dimension": null}],
        "dimensions": [
            {"unique_name": "time.calendar.[Year]", "hierarchy": "time.calendar"},
            {"unique_name": "geo.country.[Country]", "hierarchy": "geo.country"}
        ]
    })
    .to_string()
}

/// BoundMqo JSON with high-cardinality dimensions (will exceed threshold).
fn bound_mqo_high_card() -> String {
    serde_json::json!({
        "mqo": {
            "model": "sales",
            "measures": [{"unique_name": "sales.revenue"}],
            "dimensions": [
                {"hierarchy": "time.calendar", "level": "Date"},
                {"hierarchy": "product.category", "level": "Product"}
            ],
            "filters": [],
            "time_intelligence": [],
            "order": null,
            "limit": null,
            "non_empty": false
        },
        "measures": [{"unique_name": "sales.revenue", "is_calc": false, "semi_additive": false, "required_dimension": null}],
        "dimensions": [
            {"unique_name": "time.calendar.[Date]", "hierarchy": "time.calendar"},
            {"unique_name": "product.category.[Product]", "hierarchy": "product.category"}
        ]
    })
    .to_string()
}

/// StatBundle with low cardinalities (Year=5, Country=10, est=50).
fn stats_low_card() -> String {
    serde_json::json!({
        "level_cardinalities": {
            "time.calendar.[Year]": 5,
            "geo.country.[Country]": 10
        },
        "shape_flags": {
            "asymmetric_axes": false,
            "drill_through": false,
            "cellset_requested": false
        }
    })
    .to_string()
}

/// StatBundle with high cardinalities (Date=1000, Product=200, est=200_000).
fn stats_high_card() -> String {
    serde_json::json!({
        "level_cardinalities": {
            "time.calendar.[Date]": 1000,
            "product.category.[Product]": 200
        },
        "shape_flags": {
            "asymmetric_axes": false,
            "drill_through": false,
            "cellset_requested": false
        }
    })
    .to_string()
}

/// StatBundle with drill_through flag set.
fn stats_drill_through() -> String {
    serde_json::json!({
        "level_cardinalities": {
            "time.calendar.[Year]": 5,
            "geo.country.[Country]": 10
        },
        "shape_flags": {
            "asymmetric_axes": false,
            "drill_through": true,
            "cellset_requested": false
        }
    })
    .to_string()
}

// ── Binary locator ───────────────────────────────────────────────────────────

fn mqo_route_bin() -> std::path::PathBuf {
    // Build the binary first (cargo test --release builds deps but not bins
    // unless tests explicitly exercise the bin path via `cargo build`).
    let out = Command::new("cargo")
        .args(["build", "--release", "--bin", "mqo-route"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo build --release --bin mqo-route");
    assert!(
        out.status.success(),
        "cargo build failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.join("target/release/mqo-route")
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// AC7 — low-cardinality input routes to dax.
#[test]
fn end_to_end_dax_routing() {
    let bin = mqo_route_bin();
    let dir = TempDir::new().expect("tempdir");

    let bound_path = dir.path().join("bound.json");
    let stats_path = dir.path().join("stats.json");
    fs::write(&bound_path, bound_mqo_low_card()).expect("write bound");
    fs::write(&stats_path, stats_low_card()).expect("write stats");

    let out = Command::new(&bin)
        .args(["--bound", bound_path.to_str().unwrap(), "--stats", stats_path.to_str().unwrap()])
        .output()
        .expect("mqo-route");

    assert!(
        out.status.success(),
        "exit code {:?}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    let decision: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON: {e}\nstdout: {stdout}"));

    assert_eq!(decision["backend"], "dax", "expected dax routing, got: {decision}");
    assert_eq!(decision["estimated_rows"], 50, "expected 5*10=50");
    assert!(
        decision.get("sql_projection").is_none() || decision["sql_projection"].is_null(),
        "dax routing must not emit sql_projection"
    );
}

/// AC7 — high-cardinality input routes to sql with a non-empty sql_projection.
#[test]
fn end_to_end_sql_routing() {
    let bin = mqo_route_bin();
    let dir = TempDir::new().expect("tempdir");

    let bound_path = dir.path().join("bound.json");
    let stats_path = dir.path().join("stats.json");
    fs::write(&bound_path, bound_mqo_high_card()).expect("write bound");
    fs::write(&stats_path, stats_high_card()).expect("write stats");

    let out = Command::new(&bin)
        .args([
            "--bound", bound_path.to_str().unwrap(),
            "--stats", stats_path.to_str().unwrap(),
            "--row-threshold", "50000",
        ])
        .output()
        .expect("mqo-route");

    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    let stdout = String::from_utf8_lossy(&out.stdout);
    let decision: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout not JSON: {e}\n{stdout}"));

    assert_eq!(decision["backend"], "sql", "expected sql routing");
    assert_eq!(decision["estimated_rows"], 200_000_u64, "expected 1000*200=200000");
    let proj = decision["sql_projection"].as_str().expect("sql_projection must be a string");
    assert!(!proj.is_empty(), "sql_projection must be non-empty");
    assert!(proj.starts_with("SELECT "), "sql_projection must start with SELECT");
    assert!(proj.contains("\"revenue\""), "projection must include measure");
}

/// AC7 — drill-through flag routes to mdx regardless of cardinality.
#[test]
fn end_to_end_mdx_routing() {
    let bin = mqo_route_bin();
    let dir = TempDir::new().expect("tempdir");

    let bound_path = dir.path().join("bound.json");
    let stats_path = dir.path().join("stats.json");
    fs::write(&bound_path, bound_mqo_low_card()).expect("write bound");
    fs::write(&stats_path, stats_drill_through()).expect("write stats");

    let out = Command::new(&bin)
        .args(["--bound", bound_path.to_str().unwrap(), "--stats", stats_path.to_str().unwrap()])
        .output()
        .expect("mqo-route");

    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    let stdout = String::from_utf8_lossy(&out.stdout);
    let decision: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout not JSON: {e}\n{stdout}"));

    assert_eq!(decision["backend"], "mdx", "drill-through must route to mdx");
    assert!(
        decision["reason"].as_str().unwrap_or("").contains("drill-through"),
        "reason must mention drill-through"
    );
    assert!(
        decision.get("sql_projection").is_none() || decision["sql_projection"].is_null(),
        "mdx routing must not emit sql_projection"
    );
}

/// Binary must exit 2 and print to stderr when --bound file does not exist.
#[test]
fn error_on_missing_bound_file() {
    let bin = mqo_route_bin();
    let dir = TempDir::new().expect("tempdir");
    let stats_path = dir.path().join("stats.json");
    fs::write(&stats_path, stats_low_card()).expect("write stats");

    let out = Command::new(&bin)
        .args([
            "--bound", "/nonexistent/path/bound.json",
            "--stats", stats_path.to_str().unwrap(),
        ])
        .output()
        .expect("mqo-route");

    assert_eq!(
        out.status.code(),
        Some(2),
        "must exit 2 on missing --bound file"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot read --bound"),
        "must print diagnostic to stderr; got: {stderr}"
    );
}

/// Binary must exit 2 and print RouterError::NoMeasures when measures is empty.
#[test]
fn error_on_no_measures_binary() {
    let bin = mqo_route_bin();
    let dir = TempDir::new().expect("tempdir");

    let bound_path = dir.path().join("bound.json");
    let stats_path = dir.path().join("stats.json");
    // BoundMqo with empty measures list.
    let no_measures_bound = serde_json::json!({
        "mqo": {
            "model": "sales",
            "measures": [],
            "dimensions": [],
            "filters": [],
            "time_intelligence": [],
            "order": null,
            "limit": null,
            "non_empty": false
        },
        "measures": [],
        "dimensions": []
    })
    .to_string();
    fs::write(&bound_path, no_measures_bound).expect("write bound");
    fs::write(&stats_path, stats_low_card()).expect("write stats");

    let out = Command::new(&bin)
        .args(["--bound", bound_path.to_str().unwrap(), "--stats", stats_path.to_str().unwrap()])
        .output()
        .expect("mqo-route");

    assert_eq!(
        out.status.code(),
        Some(2),
        "must exit 2 on RouterError::NoMeasures"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("routing error"),
        "must print routing error to stderr; got: {stderr}"
    );
}
