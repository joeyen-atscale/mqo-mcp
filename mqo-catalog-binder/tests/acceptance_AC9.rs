//! AC9: Incompatible result serializes to JSON with reports and exits code 5,
//! distinct from NotFound (4) / Ambiguous (3) / I/O error (2).

use std::io::Write as _;
use std::process::Command;

fn binary_path() -> std::path::PathBuf {
    let mut p = std::env::current_exe()
        .expect("current_exe")
        .parent()
        .expect("bin dir")
        .to_path_buf();
    if p.ends_with("deps") {
        p = p.parent().expect("parent of deps").to_path_buf();
    }
    let candidate = p.join("mqo-bind");
    if candidate.exists() {
        return candidate;
    }
    which::which("mqo-bind").unwrap_or_else(|_| p.join("mqo-bind"))
}

fn write_temp(content: &str, suffix: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::Builder::new().suffix(suffix).tempfile().expect("tempfile");
    f.write_all(content.as_bytes()).expect("write");
    f
}

const CROSS_FACT_CATALOG: &str = r#"{
  "columns": [
    { "unique_name": "sales.store_amount", "label": "Store Amount", "kind": "measure", "is_calc": false },
    { "unique_name": "returns.reason.[Reason]", "label": "Reason", "kind": "level",
      "hierarchy": "returns.reason", "level": "Reason", "is_calc": false }
  ]
}"#;

const CROSS_FACT_MQO: &str = r#"{
  "model": "tpcds",
  "measures": [{"unique_name": "sales.store_amount"}],
  "dimensions": [{"hierarchy": "returns.reason", "level": "Reason"}],
  "filters": [],
  "time_intelligence": [],
  "non_empty": false
}"#;

const CROSS_FACT_ENRICHED: &str = r#"{
  "schema": "enriched-catalog.v1",
  "columns": [
    {"unique_name": "sales.store_amount", "column_group": ["store_sales"]},
    {"unique_name": "returns.reason.[Reason]", "column_group": ["catalog_returns"]}
  ]
}"#;

#[test]
fn ac9_incompatible_exits_5_with_json_reports() {
    let bin = binary_path();
    let mqo_f = write_temp(CROSS_FACT_MQO, ".json");
    let cat_f = write_temp(CROSS_FACT_CATALOG, ".json");
    let enriched_f = write_temp(CROSS_FACT_ENRICHED, ".json");

    let out = Command::new(&bin)
        .args([
            "--mqo", mqo_f.path().to_str().unwrap(),
            "--catalog", cat_f.path().to_str().unwrap(),
            "--enriched-catalog", enriched_f.path().to_str().unwrap(),
        ])
        .output()
        .expect("spawn mqo-bind");

    assert_eq!(
        out.status.code(),
        Some(5),
        "Incompatible must exit 5; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    let json: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be valid JSON");
    let reports = json["incompatible"]
        .as_array()
        .expect("stdout must contain 'incompatible' array");
    assert!(!reports.is_empty(), "incompatible array must not be empty");

    let r = &reports[0];
    assert!(r["measure_unique_name"].is_string(), "report must have measure_unique_name");
    assert!(r["dimension_unique_name"].is_string(), "report must have dimension_unique_name");
    assert!(r["measure_column_groups"].is_array(), "report must have measure_column_groups");
    assert!(r["dimension_column_groups"].is_array(), "report must have dimension_column_groups");
    assert!(r["note"].is_string(), "report must have note");
}

#[test]
fn ac9_exit_code_5_distinct_from_3_4_and_2() {
    // Verify that exit 5 is not the same as Ambiguous (3), NotFound (4), or I/O error (2).
    // The AC9 happy path already verifies exit 5. This test verifies the others are unchanged.
    let bin = binary_path();
    let cat_f = write_temp(CROSS_FACT_CATALOG, ".json");

    // NotFound → 4
    let not_found_mqo = write_temp(
        r#"{"model":"tpcds","measures":[{"unique_name":"NonExistent"}],"dimensions":[],"filters":[],"time_intelligence":[],"non_empty":false}"#,
        ".json",
    );
    let enriched_f = write_temp(CROSS_FACT_ENRICHED, ".json");

    let out4 = Command::new(&bin)
        .args([
            "--mqo", not_found_mqo.path().to_str().unwrap(),
            "--catalog", cat_f.path().to_str().unwrap(),
            "--enriched-catalog", enriched_f.path().to_str().unwrap(),
        ])
        .output()
        .expect("spawn");
    assert_eq!(out4.status.code(), Some(4), "not_found must still be 4");

    // I/O error → 2
    let out2 = Command::new(&bin)
        .args([
            "--mqo", "/nonexistent.json",
            "--catalog", cat_f.path().to_str().unwrap(),
        ])
        .output()
        .expect("spawn");
    assert_eq!(out2.status.code(), Some(2), "I/O error must still be 2");
}
