//! AC1: Two bi-asset.v1 bundles compose into a dashboard.v1 with two panels,
//! each panel's title preserved from its source bundle.

use mqo_dashboard_composer::{build_dashboard, BiAssetBundle, Layout};

fn make_bundle(title: &str) -> BiAssetBundle {
    BiAssetBundle {
        asset: Some("bi-asset.v1".to_owned()),
        title: title.to_owned(),
        description: format!("Description for {title}"),
        vega_spec: serde_json::json!({"mark": "bar"}),
        profile_summary: None,
        caveats: vec![],
    }
}

#[test]
fn ac1_two_panels_titles_preserved() {
    let bundles = vec![
        make_bundle("Revenue by Year"),
        make_bundle("Margin by Region"),
    ];
    let dashboard = build_dashboard(&bundles, "Sales Overview", Layout::Grid, 2);

    assert_eq!(dashboard.dashboard, "dashboard.v1");
    assert_eq!(dashboard.panels.len(), 2);
    assert_eq!(dashboard.panels[0].title, "Revenue by Year");
    assert_eq!(dashboard.panels[1].title, "Margin by Region");
}
