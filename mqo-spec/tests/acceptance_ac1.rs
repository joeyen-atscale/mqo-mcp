//! AC1: All MQO types round-trip through JSON losslessly for every fixture.

use mqo_spec::Mqo;
use std::path::Path;

fn round_trip_file(path: &Path) {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));

    let parsed: Mqo = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()));

    let reserialised = serde_json::to_string(&parsed)
        .unwrap_or_else(|e| panic!("failed to serialise {}: {e}", path.display()));

    let reparsed: Mqo = serde_json::from_str(&reserialised)
        .unwrap_or_else(|e| panic!("failed to re-parse {}: {e}", path.display()));

    assert_eq!(
        parsed,
        reparsed,
        "round-trip not equal for {}",
        path.display()
    );
}

#[test]
fn round_trip_fixtures() {
    let fixtures_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut count = 0;
    for entry in std::fs::read_dir(&fixtures_dir)
        .unwrap_or_else(|e| panic!("cannot read fixtures dir: {e}"))
    {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            round_trip_file(&path);
            count += 1;
        }
    }
    assert!(count >= 6, "expected ≥6 fixtures, found {count}");
}
