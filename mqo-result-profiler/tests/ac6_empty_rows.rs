//! AC6: empty rows array → row_count = 0, columns still typed from bound/catalog, no panic.

use mqo_result_profiler::{profile, DataType, Role};

#[test]
fn ac6_empty_rows_no_panic() {
    let response = serde_json::json!({
        "rows": [],
        "bound": {
            "measures": ["revenue"],
            "dimensions": ["region", "year"]
        }
    });
    let catalog = serde_json::json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "region",  "label": "Region",  "kind": "dimension"},
            {
                "unique_name": "year",
                "label": "Year",
                "kind": "dimension",
                "hierarchy": "time.calendar"
            }
        ]
    });

    let p = profile(&response, &catalog).expect("profile should succeed with empty rows");

    assert_eq!(p.row_count, 0);
    assert_eq!(p.measure_count, 1);
    assert_eq!(p.dimension_count, 2);
    assert_eq!(p.columns.len(), 3);

    let rev = p.columns.iter().find(|c| c.name == "revenue").unwrap();
    assert_eq!(rev.role, Role::Measure);
    assert_eq!(rev.data_type, DataType::Quantitative);
    assert_eq!(rev.cardinality, 0);
    assert!((rev.null_rate - 0.0).abs() < 1e-9);
    assert!(rev.measure_range.is_none());

    let reg = p.columns.iter().find(|c| c.name == "region").unwrap();
    assert_eq!(reg.role, Role::Dimension);
    assert_eq!(reg.data_type, DataType::Nominal);

    let yr = p.columns.iter().find(|c| c.name == "year").unwrap();
    assert_eq!(yr.role, Role::Dimension);
    assert_eq!(yr.data_type, DataType::Temporal);
}
