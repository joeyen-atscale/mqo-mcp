//! AC7: columns in bound projection order; serialization stable across runs.

use mqo_result_profiler::profile;

#[test]
fn ac7_column_order_follows_bound_projection() {
    // bound: measures first, then dimensions — and in the order listed
    let response = serde_json::json!({
        "rows": [
            {"revenue": 100.0, "cost": 60.0, "region": "EMEA", "year": 2021}
        ],
        "bound": {
            "measures": ["revenue", "cost"],
            "dimensions": ["region", "year"]
        }
    });
    let catalog = serde_json::json!({"columns": [
        {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
        {"unique_name": "cost",    "label": "Cost",    "kind": "measure"},
        {"unique_name": "region",  "label": "Region",  "kind": "dimension"},
        {"unique_name": "year",    "label": "Year",    "kind": "dimension", "hierarchy": "time.calendar"}
    ]});

    let p = profile(&response, &catalog).expect("profile should succeed");
    let names: Vec<&str> = p.columns.iter().map(|c| c.name.as_str()).collect();
    // measures come before dimensions, both in listed order
    assert_eq!(names, vec!["revenue", "cost", "region", "year"]);
}

#[test]
fn ac7_serialization_is_stable() {
    let response = serde_json::json!({
        "rows": [{"revenue": 100.0, "region": "EMEA"}],
        "bound": {"measures": ["revenue"], "dimensions": ["region"]}
    });
    let catalog = serde_json::json!({"columns": [
        {"unique_name": "revenue", "label": "Revenue", "kind": "measure"},
        {"unique_name": "region",  "label": "Region",  "kind": "dimension"}
    ]});

    let p1 = profile(&response, &catalog).unwrap();
    let p2 = profile(&response, &catalog).unwrap();

    let s1 = serde_json::to_string(&p1).unwrap();
    let s2 = serde_json::to_string(&p2).unwrap();
    assert_eq!(s1, s2, "serialization must be stable across runs");
}
