//! AC6: One panel → valid single-panel dashboard.v1 with one entry in panels
//! and a one-element concat array.

use mqo_dashboard_composer::{build_dashboard, BiAssetBundle, Layout};

#[test]
fn ac6_single_panel_valid() {
    let bundles = vec![BiAssetBundle {
        asset: Some("bi-asset.v1".to_owned()),
        title: "Solo Panel".to_owned(),
        description: "The only chart".to_owned(),
        vega_spec: serde_json::json!({"mark": "arc"}),
        profile_summary: None,
        caveats: vec![],
    }];

    let d = build_dashboard(&bundles, "Solo Dashboard", Layout::Grid, 2);

    assert_eq!(d.panels.len(), 1, "exactly one panel");
    assert_eq!(d.panels[0].title, "Solo Panel");

    let concat = d.vega_concat_spec["concat"]
        .as_array()
        .expect("concat array");
    assert_eq!(concat.len(), 1, "one-element concat array");
    assert_eq!(concat[0]["mark"], "arc");
}

#[test]
fn ac6_single_panel_vertical() {
    let bundles = vec![BiAssetBundle {
        asset: Some("bi-asset.v1".to_owned()),
        title: "Solo".to_owned(),
        description: "desc".to_owned(),
        vega_spec: serde_json::json!({"mark": "bar"}),
        profile_summary: None,
        caveats: vec![],
    }];

    let d = build_dashboard(&bundles, "T", Layout::Vertical, 1);
    let vconcat = d.vega_concat_spec["vconcat"]
        .as_array()
        .expect("vconcat array");
    assert_eq!(vconcat.len(), 1);
}
