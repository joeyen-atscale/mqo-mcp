//! AC2: dimension in time.* hierarchy → Temporal, even with integer row values.

use mqo_result_profiler::{profile, DataType, Role};

#[test]
fn ac2_temporal_from_hierarchy() {
    let response = serde_json::json!({
        "rows": [
            {"revenue": 100.0, "year": 2021},
            {"revenue": 200.0, "year": 2022}
        ],
        "bound": {
            "measures": ["revenue"],
            "dimensions": ["year"]
        }
    });
    let catalog = serde_json::json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {
                "unique_name": "year",
                "label": "Year",
                "kind": "dimension",
                "hierarchy": "time.calendar"
            }
        ]
    });

    let p = profile(&response, &catalog).expect("profile should succeed");
    let yr = p.columns.iter().find(|c| c.name == "year").expect("year column");
    assert_eq!(yr.role, Role::Dimension);
    // Must be Temporal despite integer values, because hierarchy = "time.calendar"
    assert_eq!(yr.data_type, DataType::Temporal);
}

#[test]
fn ac2_temporal_from_date_string_values_fallback() {
    // When the catalog hierarchy is absent, fallback to value inspection.
    let response = serde_json::json!({
        "rows": [
            {"revenue": 100.0, "date": "2021-01-01"},
            {"revenue": 200.0, "date": "2022-06-15"}
        ],
        "bound": {
            "measures": ["revenue"],
            "dimensions": ["date"]
        }
    });
    let catalog = serde_json::json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "date", "label": "Date", "kind": "dimension"}
        ]
    });

    let p = profile(&response, &catalog).expect("profile should succeed");
    let dt_col = p.columns.iter().find(|c| c.name == "date").expect("date column");
    assert_eq!(dt_col.data_type, DataType::Temporal);
}
