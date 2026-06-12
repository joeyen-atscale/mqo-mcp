//! AC3: Capability enumerates exactly the nine ops named in the PRD;
//! a test asserts the set.

use dh_spec::{Capability, ALL_CAPABILITIES};
use std::collections::HashSet;

#[test]
fn ac3_exactly_nine_capabilities() {
    assert_eq!(
        ALL_CAPABILITIES.len(),
        9,
        "Capability must have exactly 9 variants"
    );
}

#[test]
fn ac3_all_named_ops_present() {
    let caps: HashSet<&str> = ALL_CAPABILITIES
        .iter()
        .map(|c| match c {
            Capability::Aggregate => "Aggregate",
            Capability::Filter => "Filter",
            Capability::Sort => "Sort",
            Capability::TopN => "TopN",
            Capability::Pivot => "Pivot",
            Capability::Compare => "Compare",
            Capability::Drill => "Drill",
            Capability::Describe => "Describe",
            Capability::Export => "Export",
        })
        .collect();

    let expected: HashSet<&str> = [
        "Aggregate", "Filter", "Sort", "TopN", "Pivot",
        "Compare", "Drill", "Describe", "Export",
    ]
    .iter()
    .copied()
    .collect();

    assert_eq!(caps, expected, "ALL_CAPABILITIES must match the PRD-specified nine ops exactly");
}

#[test]
fn ac3_capability_serializes_to_pascal_case() {
    // Each capability should round-trip through JSON as its PascalCase name.
    for cap in ALL_CAPABILITIES {
        let json = serde_json::to_string(&cap).expect("serialize Capability");
        // Strip quotes.
        let name = json.trim_matches('"');
        // PascalCase: first char uppercase, no underscores.
        assert!(
            name.chars().next().map_or(false, |c| c.is_uppercase()),
            "Capability {cap:?} must serialize with uppercase first letter, got {json}"
        );
        assert!(
            !name.contains('_'),
            "Capability {cap:?} must not serialize with underscores, got {json}"
        );
        // Must round-trip.
        let reparsed: Capability = serde_json::from_str(&json).expect("deserialize Capability");
        assert_eq!(reparsed, cap);
    }
}
