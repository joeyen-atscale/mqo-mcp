//! AC4: Each panel caption incorporates the source bundle's description and
//! any caveats.

use mqo_dashboard_composer::{build_dashboard, BiAssetBundle, Layout};

fn make_bundle_with_caveats(description: &str, caveats: Vec<&str>) -> BiAssetBundle {
    BiAssetBundle {
        asset: Some("bi-asset.v1".to_owned()),
        title: "My Panel".to_owned(),
        description: description.to_owned(),
        vega_spec: serde_json::json!({"mark": "bar"}),
        profile_summary: None,
        caveats: caveats.into_iter().map(String::from).collect(),
    }
}

#[test]
fn ac4_caption_description_only() {
    let bundles = vec![make_bundle_with_caveats("Revenue by fiscal year", vec![])];
    let d = build_dashboard(&bundles, "T", Layout::Grid, 2);
    assert_eq!(d.panels[0].caption, "Revenue by fiscal year");
}

#[test]
fn ac4_caption_with_one_caveat() {
    let bundles = vec![make_bundle_with_caveats(
        "Revenue by fiscal year",
        vec!["balance measures are semi-additive and are not summed over time"],
    )];
    let d = build_dashboard(&bundles, "T", Layout::Grid, 2);
    let caption = &d.panels[0].caption;
    assert!(
        caption.contains("Revenue by fiscal year"),
        "caption should contain description"
    );
    assert!(
        caption.contains("balance measures"),
        "caption should contain caveat text"
    );
}

#[test]
fn ac4_caption_with_multiple_caveats() {
    let bundles = vec![make_bundle_with_caveats(
        "Sales summary",
        vec!["caveat one", "caveat two"],
    )];
    let d = build_dashboard(&bundles, "T", Layout::Grid, 2);
    let caption = &d.panels[0].caption;
    assert!(caption.contains("Sales summary"));
    assert!(caption.contains("caveat one"));
    assert!(caption.contains("caveat two"));
}
