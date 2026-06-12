//! Integration tests for the `mqo-bind` CLI binary.
//!
//! These tests exercise the binary via `std::process::Command`, verifying
//! stdout JSON shape and exit codes without depending on the library internals.

use std::io::Write;
use std::process::Command;

fn binary_path() -> std::path::PathBuf {
    // Prefer `cargo test` artifact path; fall back to PATH.
    let mut p = std::env::current_exe()
        .expect("current_exe")
        .parent()
        .expect("bin dir")
        .to_path_buf();
    // strip deps/ suffix when running from cargo test
    if p.ends_with("deps") {
        p = p.parent().expect("parent of deps").to_path_buf();
    }
    let candidate = p.join("mqo-bind");
    if candidate.exists() {
        return candidate;
    }
    // fall back: assume it is on PATH
    which::which("mqo-bind").unwrap_or_else(|_| p.join("mqo-bind"))
}

fn write_temp(content: &str, suffix: &str) -> tempfile::NamedTempFile {
    let mut f =
        tempfile::Builder::new().suffix(suffix).tempfile().expect("tempfile");
    f.write_all(content.as_bytes()).expect("write");
    f
}

const FIXTURE_CATALOG: &str = r#"{
  "columns": [
    { "unique_name": "sales.revenue", "label": "Revenue", "kind": "measure", "is_calc": false },
    { "unique_name": "sales.units",   "label": "Units",   "kind": "measure", "is_calc": false },
    { "unique_name": "time.calendar.[Year]", "label": "Year", "kind": "level",
      "hierarchy": "time.calendar", "level": "Year", "is_calc": false }
  ],
  "describe_model": {
    "calc_groups": [
      { "group_name": "TI", "member_name": "YTD", "unique_name": "calc.ti.YTD",
        "mdx": "Aggregate(PeriodsToDate(...))" }
    ]
  }
}"#;

fn simple_mqo(measure: &str) -> String {
    format!(
        r#"{{
  "model": "sales",
  "measures": [{{"unique_name": "{measure}"}}],
  "dimensions": [],
  "filters": [],
  "time_intelligence": [],
  "non_empty": false
}}"#
    )
}

/// AC1 (CLI): valid MQO → exit 0, stdout is valid JSON with `measures`.
#[test]
fn cli_bound_exits_0() {
    let bin = binary_path();
    let mqo_f = write_temp(&simple_mqo("Revenue"), ".json");
    let cat_f = write_temp(FIXTURE_CATALOG, ".json");

    let out = Command::new(&bin)
        .args(["--mqo", mqo_f.path().to_str().unwrap(),
               "--catalog", cat_f.path().to_str().unwrap()])
        .output()
        .expect("spawn mqo-bind");

    assert_eq!(out.status.code(), Some(0), "expected exit 0, got: {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr));

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be valid JSON");
    assert!(json.get("measures").is_some(), "BoundMqo must contain 'measures' key");
    let measures = json["measures"].as_array().expect("measures array");
    assert_eq!(measures.len(), 1);
    assert_eq!(measures[0]["unique_name"], "sales.revenue");
}

/// AC2 (CLI): fabricated measure → exit 4, stdout has `not_found`.
#[test]
fn cli_not_found_exits_4() {
    let bin = binary_path();
    let mqo_f = write_temp(&simple_mqo("FakeMeasureXYZ"), ".json");
    let cat_f = write_temp(FIXTURE_CATALOG, ".json");

    let out = Command::new(&bin)
        .args(["--mqo", mqo_f.path().to_str().unwrap(),
               "--catalog", cat_f.path().to_str().unwrap()])
        .output()
        .expect("spawn mqo-bind");

    assert_eq!(out.status.code(), Some(4),
        "expected exit 4; stdout: {}", String::from_utf8_lossy(&out.stdout));

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be valid JSON");
    assert!(json.get("not_found").is_some(), "must contain 'not_found' key");
}

/// AC3 (CLI): ambiguous label → exit 3, stdout has `ambiguous`.
#[test]
fn cli_ambiguous_exits_3() {
    let catalog = r#"{
      "columns": [
        { "unique_name": "model_a.revenue", "label": "Revenue", "kind": "measure", "is_calc": false },
        { "unique_name": "model_b.revenue", "label": "Revenue", "kind": "measure", "is_calc": false }
      ]
    }"#;
    let bin = binary_path();
    let mqo_f = write_temp(&simple_mqo("Revenue"), ".json");
    let cat_f = write_temp(catalog, ".json");

    let out = Command::new(&bin)
        .args(["--mqo", mqo_f.path().to_str().unwrap(),
               "--catalog", cat_f.path().to_str().unwrap()])
        .output()
        .expect("spawn mqo-bind");

    assert_eq!(out.status.code(), Some(3),
        "expected exit 3; stdout: {}", String::from_utf8_lossy(&out.stdout));

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be valid JSON");
    assert!(json.get("ambiguous").is_some(), "must contain 'ambiguous' key");
}

/// Exit 2: bad --mqo path.
#[test]
fn cli_bad_mqo_path_exits_2() {
    let bin = binary_path();
    let cat_f = write_temp(FIXTURE_CATALOG, ".json");

    let out = Command::new(&bin)
        .args(["--mqo", "/nonexistent/path/mqo.json",
               "--catalog", cat_f.path().to_str().unwrap()])
        .output()
        .expect("spawn mqo-bind");

    assert_eq!(out.status.code(), Some(2),
        "expected exit 2 for bad --mqo path");
}

/// Exit 2: bad --catalog path.
#[test]
fn cli_bad_catalog_path_exits_2() {
    let bin = binary_path();
    let mqo_f = write_temp(&simple_mqo("Revenue"), ".json");

    let out = Command::new(&bin)
        .args(["--mqo", mqo_f.path().to_str().unwrap(),
               "--catalog", "/nonexistent/catalog.json"])
        .output()
        .expect("spawn mqo-bind");

    assert_eq!(out.status.code(), Some(2),
        "expected exit 2 for bad --catalog path");
}
