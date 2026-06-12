//! AC1: bound measures → Measure/Quantitative; bound dimensions → Dimension.

use mqo_result_profiler::{profile, DataType, Role};

#[test]
fn ac1_basic_roles() {
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
            {"unique_name": "year", "label": "Year", "kind": "dimension"}
        ]
    });

    let p = profile(&response, &catalog).expect("profile should succeed");
    assert_eq!(p.row_count, 2);
    assert_eq!(p.measure_count, 1);
    assert_eq!(p.dimension_count, 1);

    let rev = p.columns.iter().find(|c| c.name == "revenue").expect("revenue column");
    assert_eq!(rev.role, Role::Measure);
    assert_eq!(rev.data_type, DataType::Quantitative);
    assert_eq!(rev.label, "Revenue");

    let yr = p.columns.iter().find(|c| c.name == "year").expect("year column");
    assert_eq!(yr.role, Role::Dimension);
}
