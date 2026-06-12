//! AC3: Vertical layout emits a vega_concat_spec whose `vconcat` array holds
//! the panel vega_specs in input order.

use mqo_dashboard_composer::{build_dashboard, BiAssetBundle, Layout};

fn make_bundle(title: &str, mark: &str) -> BiAssetBundle {
    BiAssetBundle {
        asset: Some("bi-asset.v1".to_owned()),
        title: title.to_owned(),
        description: "d".to_owned(),
        vega_spec: serde_json::json!({"mark": mark, "title": title}),
        profile_summary: None,
        caveats: vec![],
    }
}

#[test]
fn ac3_vconcat_order_preserved() {
    let bundles = vec![
        make_bundle("First", "bar"),
        make_bundle("Second", "line"),
        make_bundle("Third", "point"),
    ];
    let dashboard = build_dashboard(&bundles, "Stacked", Layout::Vertical, 1);

    // Should have vconcat, not hconcat or concat
    let vconcat = dashboard
        .vega_concat_spec
        .get("vconcat")
        .expect("vconcat key must be present")
        .as_array()
        .expect("vconcat must be an array");

    assert!(dashboard.vega_concat_spec.get("hconcat").is_none());
    assert!(dashboard.vega_concat_spec.get("concat").is_none());

    // Three specs in input order
    assert_eq!(vconcat.len(), 3);
    assert_eq!(vconcat[0]["mark"], "bar");
    assert_eq!(vconcat[1]["mark"], "line");
    assert_eq!(vconcat[2]["mark"], "point");
}
