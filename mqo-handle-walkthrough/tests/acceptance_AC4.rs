//! AC4: period_over_period adds a yoy_change column with correct values.
//! Uses fixture rows where both 2022 and 2023 rows are present so we can
//! assert the delta arithmetic.

use mqo_handle_walkthrough::ops;

#[test]
fn ac4_yoy_change_column_values() {
    // Minimal fixture with known numbers.
    let rows: Vec<serde_json::Value> = serde_json::from_str(r#"[
        {"state": "California", "month": "2022-01", "web_sales": 105000.0, "year": 2022},
        {"state": "California", "month": "2023-01", "web_sales": 120000.0, "year": 2023}
    ]"#)
    .unwrap();

    let result = ops::period_over_period(&rows);

    // Find the 2023 California Jan row.
    let ca_2023 = result
        .iter()
        .find(|r| {
            r.get("year").and_then(|v| v.as_f64()).map(|y| y as i64) == Some(2023)
                && r.get("state").and_then(|v| v.as_str()) == Some("California")
                && r.get("month").and_then(|v| v.as_str()) == Some("2023-01")
        })
        .expect("must have 2023 California Jan");

    let yoy = ca_2023
        .get("yoy_change")
        .and_then(|v| v.as_f64())
        .expect("yoy_change must be numeric for matched row");

    // 120000 - 105000 = 15000
    assert!(
        (yoy - 15000.0).abs() < 0.01,
        "yoy_change must be 15000 for California Jan 2023, got {yoy}"
    );
}

#[test]
fn ac4_yoy_change_null_when_no_prior_year() {
    let rows: Vec<serde_json::Value> = serde_json::from_str(r#"[
        {"state": "Oregon", "month": "2023-03", "web_sales": 50000.0, "year": 2023}
    ]"#)
    .unwrap();

    let result = ops::period_over_period(&rows);
    let oregon = &result[0];
    let yoy = oregon.get("yoy_change").expect("yoy_change key must exist");
    assert!(
        yoy.is_null(),
        "yoy_change must be null when no prior-year row exists, got {yoy:?}"
    );
}

#[test]
fn ac4_prior_year_rows_get_null_yoy() {
    let rows: Vec<serde_json::Value> = serde_json::from_str(r#"[
        {"state": "California", "month": "2022-01", "web_sales": 105000.0, "year": 2022},
        {"state": "California", "month": "2023-01", "web_sales": 120000.0, "year": 2023}
    ]"#)
    .unwrap();

    let result = ops::period_over_period(&rows);
    let prior = result
        .iter()
        .find(|r| r.get("year").and_then(|v| v.as_f64()).map(|y| y as i64) == Some(2022))
        .unwrap();
    assert!(
        prior.get("yoy_change").map(|v| v.is_null()).unwrap_or(false),
        "prior-year rows must have null yoy_change"
    );
}
