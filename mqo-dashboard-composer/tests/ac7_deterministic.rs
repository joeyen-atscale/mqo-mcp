//! AC7: Panel ordering and grid placement are deterministic across repeated runs
//! on identical input.

use mqo_dashboard_composer::{build_dashboard, BiAssetBundle, Layout};

fn make_bundle(title: &str) -> BiAssetBundle {
    BiAssetBundle {
        asset: Some("bi-asset.v1".to_owned()),
        title: title.to_owned(),
        description: format!("desc for {title}"),
        vega_spec: serde_json::json!({"mark": "line", "title": title}),
        profile_summary: None,
        caveats: vec!["note A".to_owned()],
    }
}

#[test]
fn ac7_repeated_runs_identical_output() {
    let bundles = vec![
        make_bundle("Revenue"),
        make_bundle("Margin"),
        make_bundle("Orders"),
    ];

    let d1 = build_dashboard(&bundles, "My Dash", Layout::Grid, 2);
    let d2 = build_dashboard(&bundles, "My Dash", Layout::Grid, 2);
    let d3 = build_dashboard(&bundles, "My Dash", Layout::Grid, 2);

    let j1 = serde_json::to_string(&d1).expect("serialize run 1");
    let j2 = serde_json::to_string(&d2).expect("serialize run 2");
    let j3 = serde_json::to_string(&d3).expect("serialize run 3");

    assert_eq!(j1, j2, "run 1 and 2 must be identical");
    assert_eq!(j2, j3, "run 2 and 3 must be identical");
}

#[test]
fn ac7_panel_order_matches_input_order() {
    let titles = vec!["First", "Second", "Third", "Fourth"];
    let bundles: Vec<_> = titles.iter().map(|t| make_bundle(t)).collect();
    let d = build_dashboard(&bundles, "Ordered", Layout::Grid, 2);

    for (i, title) in titles.iter().enumerate() {
        assert_eq!(&d.panels[i].title, title, "panel {i} title must match input order");
    }
}
