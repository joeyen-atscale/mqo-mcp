//! AC4: At least 6 golden fixtures parse and validate successfully.

use mqo_spec::{Mqo, validate};
use std::path::Path;

fn load_and_validate(path: &Path) {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));

    let mqo: Mqo = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));

    validate(&mqo).unwrap_or_else(|errs| {
        panic!(
            "fixture {} failed validation: {errs:?}",
            path.display()
        )
    });
}

#[test]
fn all_golden_fixtures_parse_and_validate() {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut count = 0;

    let mut paths: Vec<_> = std::fs::read_dir(&fixtures_dir)
        .unwrap_or_else(|e| panic!("cannot read fixtures dir: {e}"))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    paths.sort();

    for path in &paths {
        load_and_validate(path);
        count += 1;
    }

    assert!(
        count >= 6,
        "expected ≥6 fixtures to parse and validate, found {count}"
    );
}

#[test]
fn fixture_01_yoy_has_yoy_time_intel() {
    use mqo_spec::TimeIntel;
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/01_yoy.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    let mqo: Mqo = serde_json::from_str(&raw).unwrap();
    assert!(
        mqo.time_intelligence.iter().any(|t| matches!(t, TimeIntel::YoY)),
        "01_yoy fixture should have YoY time intel"
    );
}

#[test]
fn fixture_07_has_calc_group_filter() {
    use mqo_spec::Filter;
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/07_calc_group_filter.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    let mqo: Mqo = serde_json::from_str(&raw).unwrap();
    assert!(
        mqo.filters
            .iter()
            .any(|f| matches!(f, Filter::CalcGroupMember { .. })),
        "07_calc_group_filter fixture should have CalcGroupMember filter"
    );
}

#[test]
fn fixture_08_has_member_filter() {
    use mqo_spec::Filter;
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/08_member_filter.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    let mqo: Mqo = serde_json::from_str(&raw).unwrap();
    assert!(
        mqo.filters
            .iter()
            .any(|f| matches!(f, Filter::Member { .. })),
        "08_member_filter fixture should have Member filter"
    );
}
