//! AC2: Grid layout with columns=2 places:
//!   panel 0 at (row 0, col 0)
//!   panel 1 at (row 0, col 1)
//!   panel 2 at (row 1, col 0)

use mqo_dashboard_composer::{build_dashboard, BiAssetBundle, Layout};

fn make_bundle(n: u32) -> BiAssetBundle {
    BiAssetBundle {
        asset: Some("bi-asset.v1".to_owned()),
        title: format!("Panel {n}"),
        description: format!("desc {n}"),
        vega_spec: serde_json::json!({"mark": "bar"}),
        profile_summary: None,
        caveats: vec![],
    }
}

#[test]
fn ac2_grid_placement_columns_2() {
    let bundles: Vec<_> = (0..3).map(make_bundle).collect();
    let dashboard = build_dashboard(&bundles, "Test Grid", Layout::Grid, 2);

    assert_eq!(dashboard.panels.len(), 3);
    assert_eq!((dashboard.panels[0].row, dashboard.panels[0].col), (0, 0));
    assert_eq!((dashboard.panels[1].row, dashboard.panels[1].col), (0, 1));
    assert_eq!((dashboard.panels[2].row, dashboard.panels[2].col), (1, 0));
}
