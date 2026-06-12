//! AC2: FixtureEngine determinism tests.
//!
//! - Same input → same rows.
//! - Scalar (no-dim) bound → exactly 1 row.
//! - Row count = min(limit | DEFAULT_ROWS, HARD_ROW_CAP).

use mqo_auth_bridge::{Backend, Engine, EngineResult, FixtureEngine, HARD_ROW_CAP};
use serde_json::json;

fn bound_with(dims: &[&str], measures: &[&str]) -> serde_json::Value {
    json!({
        "dimensions": dims.iter().map(|d| json!({"unique_name": d, "hierarchy": "h"})).collect::<Vec<_>>(),
        "measures": measures.iter().map(|m| json!({"unique_name": m})).collect::<Vec<_>>(),
    })
}

#[test]
fn scalar_aggregate_is_one_row() {
    let eng = FixtureEngine::with_bound(bound_with(&[], &["sales.revenue"]));
    let r = eng
        .execute("SELECT ...", Backend::Dax, Some(100), None)
        .expect("fixture execute should not fail");
    assert_eq!(r.rows.len(), 1, "scalar (no dims) must always be 1 row");
    assert!(
        r.rows[0].get("sales.revenue").is_some(),
        "measure column must be present"
    );
}

#[test]
fn default_rows_when_no_limit() {
    let eng = FixtureEngine::with_bound(bound_with(&["time.[Year]"], &["sales.revenue"]));
    let r = eng
        .execute("SELECT ...", Backend::Dax, None, None)
        .expect("fixture execute should not fail");
    // DEFAULT_ROWS = 5
    assert_eq!(r.rows.len(), 5);
}

#[test]
fn limit_is_respected() {
    let eng = FixtureEngine::with_bound(bound_with(&["time.[Year]"], &["sales.revenue"]));
    let r = eng
        .execute("SELECT ...", Backend::Dax, Some(3), None)
        .expect("fixture execute should not fail");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn hard_cap_enforced() {
    let eng = FixtureEngine::with_bound(bound_with(&["time.[Year]"], &["sales.revenue"]));
    let r = eng
        .execute("SELECT ...", Backend::Dax, Some(100_000), None)
        .expect("fixture execute should not fail");
    assert_eq!(r.rows.len(), HARD_ROW_CAP);
}

#[test]
fn deterministic_same_input_same_output() {
    let bound = bound_with(&["time.[Year]"], &["sales.revenue", "sales.units"]);
    let eng = FixtureEngine::with_bound(bound.clone());
    let r1 = eng
        .execute("q", Backend::Dax, Some(5), None)
        .expect("first call should not fail");
    let r2 = eng
        .execute("q", Backend::Dax, Some(5), None)
        .expect("second call should not fail");
    assert_eq!(r1.rows, r2.rows, "fixture must be deterministic");
}

#[test]
fn row_has_dim_and_measure_columns() {
    let eng = FixtureEngine::with_bound(bound_with(
        &["time.[Year]"],
        &["sales.revenue", "sales.units"],
    ));
    let r = eng
        .execute("SELECT ...", Backend::Dax, Some(2), None)
        .expect("fixture execute should not fail");
    let row0 = &r.rows[0];
    assert!(row0.get("time.[Year]").is_some());
    assert!(row0.get("sales.revenue").is_some());
    assert!(row0.get("sales.units").is_some());
}

#[test]
fn no_bound_single_measure_column() {
    let eng = FixtureEngine::new();
    let r = eng
        .execute("SELECT revenue FROM cube", Backend::Sql, Some(3), None)
        .expect("fixture execute without bound should not fail");
    // No dims → scalar → 1 row
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn engine_result_not_capped_for_fixture() {
    let eng = FixtureEngine::with_bound(bound_with(&["time.[Year]"], &["sales.revenue"]));
    let r: EngineResult = eng
        .execute("SELECT ...", Backend::Dax, Some(5), None)
        .expect("fixture execute should not fail");
    assert!(
        !r.row_cap_tripped,
        "fixture engine never trips the row cap flag"
    );
}
