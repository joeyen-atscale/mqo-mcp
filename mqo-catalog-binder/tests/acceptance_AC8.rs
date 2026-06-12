//! AC8: Missing or malformed --enriched-catalog → non-zero exit with stderr diagnostic,
//! does NOT fall back to the no-check path.

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

const FIXTURE_CATALOG: &str = r#"{
  "columns": [
    { "unique_name": "sales.revenue", "label": "Revenue", "kind": "measure", "is_calc": false }
  ]
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

#[test]
fn ac8_missing_enriched_catalog_file_exits_nonzero() {
    let bin = binary_path();
    let mqo_f = write_temp(&simple_mqo("Revenue"), ".json");
    let cat_f = write_temp(FIXTURE_CATALOG, ".json");

    let out = Command::new(&bin)
        .args([
            "--mqo", mqo_f.path().to_str().unwrap(),
            "--catalog", cat_f.path().to_str().unwrap(),
            "--enriched-catalog", "/nonexistent/enriched.json",
        ])
        .output()
        .expect("spawn mqo-bind");

    assert_ne!(
        out.status.code(),
        Some(0),
        "missing enriched-catalog must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.is_empty(),
        "missing enriched-catalog must emit a stderr diagnostic"
    );
}

#[test]
fn ac8_malformed_enriched_catalog_exits_nonzero() {
    let bin = binary_path();
    let mqo_f = write_temp(&simple_mqo("Revenue"), ".json");
    let cat_f = write_temp(FIXTURE_CATALOG, ".json");
    let bad_enriched = write_temp("THIS IS NOT JSON {{{", ".json");

    let out = Command::new(&bin)
        .args([
            "--mqo", mqo_f.path().to_str().unwrap(),
            "--catalog", cat_f.path().to_str().unwrap(),
            "--enriched-catalog", bad_enriched.path().to_str().unwrap(),
        ])
        .output()
        .expect("spawn mqo-bind");

    assert_ne!(
        out.status.code(),
        Some(0),
        "malformed enriched-catalog must exit non-zero"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.is_empty(),
        "malformed enriched-catalog must emit a stderr diagnostic"
    );
    // Must NOT have fallen back to the no-check path (which would exit 0 with measures).
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.is_empty() || !stdout.contains("\"measures\""),
        "malformed enriched-catalog must not fall back to Bound result"
    );
}
