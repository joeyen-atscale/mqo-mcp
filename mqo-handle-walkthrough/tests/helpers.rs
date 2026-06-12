//! Shared test helpers.

use serde_json::Value;

/// Load the fixture seed rows.
pub fn load_fixture() -> Vec<Value> {
    let raw = include_str!("../fixtures/seed_result.json");
    serde_json::from_str(raw).expect("fixture is valid JSON")
}
