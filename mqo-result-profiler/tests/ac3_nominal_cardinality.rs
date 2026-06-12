//! AC3: non-temporal string dimension → Nominal; cardinality = distinct non-null count.

use mqo_result_profiler::{profile, DataType, Role};

#[test]
fn ac3_nominal_cardinality() {
    let response = serde_json::json!({
        "rows": [
            {"revenue": 100.0, "region": "EMEA"},
            {"revenue": 200.0, "region": "AMER"},
            {"revenue": 150.0, "region": "EMEA"},
            {"revenue": 50.0,  "region": "APAC"}
        ],
        "bound": {
            "measures": ["revenue"],
            "dimensions": ["region"]
        }
    });
    let catalog = serde_json::json!({
        "columns": [
            {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
            {"unique_name": "region", "label": "Region", "kind": "dimension"}
        ]
    });

    let p = profile(&response, &catalog).expect("profile should succeed");
    let reg = p.columns.iter().find(|c| c.name == "region").expect("region column");

    assert_eq!(reg.role, Role::Dimension);
    assert_eq!(reg.data_type, DataType::Nominal);
    // 3 distinct values: EMEA, AMER, APAC
    assert_eq!(reg.cardinality, 3);
}

#[test]
fn ac3_cardinality_excludes_nulls() {
    let response = serde_json::json!({
        "rows": [
            {"revenue": 100.0, "region": "EMEA"},
            {"revenue": 200.0, "region": null},
            {"revenue": 150.0, "region": "AMER"}
        ],
        "bound": {
            "measures": ["revenue"],
            "dimensions": ["region"]
        }
    });
    let catalog = serde_json::json!({"columns": [
        {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
        {"unique_name": "region", "label": "Region", "kind": "dimension"}
    ]});

    let p = profile(&response, &catalog).expect("profile should succeed");
    let reg = p.columns.iter().find(|c| c.name == "region").unwrap();
    // 2 distinct non-null: EMEA, AMER
    assert_eq!(reg.cardinality, 2);
    // null_rate = 1/3
    assert!((reg.null_rate - 1.0 / 3.0).abs() < 1e-9);
}
