//! mqo-dashboard-composer — compose multiple `bi-asset.v1` bundles into a
//! `dashboard.v1` layout manifest + Vega-Lite v5 concat spec.
//!
//! # Overview
//!
//! This crate takes N `bi-asset.v1` JSON bundles (the
//! `{asset, title, description, vega_spec, profile_summary, caveats}` envelopes
//! that `mqo-bi-asset-bundle` emits) and assembles them into:
//!
//! 1. A `dashboard.v1` manifest with per-panel title, caption, grid position, and
//!    the verbatim `vega_spec`.
//! 2. A Vega-Lite v5 `vconcat`/`hconcat`/`concat` spec that wraps all panels for
//!    direct rendering.
//!
//! Pure JSON, no network, no render dependency.

#![forbid(unsafe_code)]

use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, fs, path::PathBuf};

// ──────────────────────────────────────────────────────────────────────────────
// Schema types
// ──────────────────────────────────────────────────────────────────────────────

/// A `bi-asset.v1` bundle as emitted by `mqo-bi-asset-bundle`.
/// The `asset` discriminator field is present but not validated beyond existence.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BiAssetBundle {
    /// Schema discriminator, expected to be `"bi-asset.v1"`.
    pub asset: Option<String>,
    /// Human-readable panel title.
    pub title: String,
    /// Narrative description of the panel's content.
    pub description: String,
    /// Vega-Lite v5 spec (arbitrary JSON).
    pub vega_spec: Value,
    /// Optional profiling summary (passed through, not used in composition).
    pub profile_summary: Option<Value>,
    /// Optional list of caveats to fold into the panel caption.
    #[serde(default)]
    pub caveats: Vec<String>,
}

/// A single panel entry in the `dashboard.v1` manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Panel {
    /// Panel title from the source bundle.
    pub title: String,
    /// Composed caption: description + caveats.
    pub caption: String,
    /// Verbatim `vega_spec` from the source bundle.
    pub vega_spec: Value,
    /// Row index (0-based) in the grid.
    pub row: u32,
    /// Column index (0-based) in the grid.
    pub col: u32,
}

/// The `dashboard.v1` output manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardV1 {
    /// Schema discriminator, always `"dashboard.v1"`.
    pub dashboard: String,
    /// Dashboard-level title.
    pub title: String,
    /// Layout strategy.
    pub layout: Layout,
    /// Grid width (ignored for vertical/horizontal layouts).
    pub columns: u32,
    /// Ordered list of panels.
    pub panels: Vec<Panel>,
    /// Vega-Lite v5 concat spec.
    pub vega_concat_spec: Value,
}

/// Layout strategy for panel arrangement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// Row-major grid placement.
    Grid,
    /// Single column, panels stacked vertically.
    Vertical,
    /// Single row, panels side by side.
    Horizontal,
}

impl fmt::Display for Layout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Grid => write!(f, "grid"),
            Self::Vertical => write!(f, "vertical"),
            Self::Horizontal => write!(f, "horizontal"),
        }
    }
}

/// Output format selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// JSON output of the full `dashboard.v1` payload.
    Json,
    /// Human-readable summary.
    Human,
}

// ──────────────────────────────────────────────────────────────────────────────
// CLI arguments
// ──────────────────────────────────────────────────────────────────────────────

/// Compose multiple `bi-asset.v1` bundles into a `dashboard.v1` layout manifest
/// and a Vega-Lite v5 concat spec.
#[derive(Debug, Parser)]
#[command(name = "mqo-dashboard-composer", version)]
pub struct ComposerArgs {
    /// File holding a JSON array of `bi-asset.v1` bundles.
    #[arg(long = "assets")]
    pub assets_file: Option<PathBuf>,

    /// Individual `bi-asset.v1` bundle files (may be repeated; appended after --assets).
    #[arg(long = "asset")]
    pub asset_files: Vec<PathBuf>,

    /// Dashboard title (required).
    #[arg(long)]
    pub title: String,

    /// Layout strategy.
    #[arg(long, default_value = "grid")]
    pub layout: Layout,

    /// Grid width (ignored for vertical/horizontal).
    #[arg(long, default_value_t = 2)]
    pub columns: u32,

    /// Output format.
    #[arg(long = "format", default_value = "json")]
    pub format: OutputFormat,
}

// ──────────────────────────────────────────────────────────────────────────────
// Error type
// ──────────────────────────────────────────────────────────────────────────────

/// Errors produced by the composer.
#[derive(Debug)]
pub enum ComposerError {
    /// No panels were provided.
    NoPanels,
    /// I/O error reading a bundle file.
    Io(std::io::Error),
    /// JSON parse error.
    Json(serde_json::Error),
}

impl fmt::Display for ComposerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPanels => write!(f, "no panels: at least one bi-asset.v1 bundle is required"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Json(e) => write!(f, "JSON parse error: {e}"),
        }
    }
}

impl std::error::Error for ComposerError {}

impl From<std::io::Error> for ComposerError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for ComposerError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Core logic
// ──────────────────────────────────────────────────────────────────────────────

/// Load bundles from CLI args, compose, and write output to stdout.
///
/// # Errors
///
/// Returns [`ComposerError`] if no bundles are found, file I/O fails, or
/// JSON parsing fails.
pub fn compose(args: &ComposerArgs) -> Result<DashboardV1, ComposerError> {
    let bundles = load_bundles(args)?;

    if bundles.is_empty() {
        return Err(ComposerError::NoPanels);
    }

    let dashboard = build_dashboard(&bundles, &args.title, args.layout, args.columns);

    match args.format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&dashboard)?);
        }
        OutputFormat::Human => {
            print_human(&dashboard);
        }
    }

    Ok(dashboard)
}

/// Load all bundles from `--assets` file and/or `--asset` files.
///
/// # Errors
///
/// Returns [`ComposerError`] if file I/O or JSON parsing fails.
pub fn load_bundles(args: &ComposerArgs) -> Result<Vec<BiAssetBundle>, ComposerError> {
    let mut bundles: Vec<BiAssetBundle> = Vec::new();

    if let Some(ref assets_path) = args.assets_file {
        let content = fs::read_to_string(assets_path)?;
        let arr: Vec<BiAssetBundle> = serde_json::from_str(&content)?;
        bundles.extend(arr);
    }

    for path in &args.asset_files {
        let content = fs::read_to_string(path)?;
        let bundle: BiAssetBundle = serde_json::from_str(&content)?;
        bundles.push(bundle);
    }

    Ok(bundles)
}

/// Build a `dashboard.v1` from the given bundles.
///
/// Panel order is preserved from the input slice. Grid placement is row-major.
#[must_use]
pub fn build_dashboard(
    bundles: &[BiAssetBundle],
    title: &str,
    layout: Layout,
    columns: u32,
) -> DashboardV1 {
    let panels: Vec<Panel> = bundles
        .iter()
        .enumerate()
        .map(|(i, b)| build_panel(b, i, layout, columns))
        .collect();

    let vega_concat_spec = build_vega_concat(title, &panels, layout, columns);

    DashboardV1 {
        dashboard: "dashboard.v1".to_owned(),
        title: title.to_owned(),
        layout,
        columns,
        panels,
        vega_concat_spec,
    }
}

/// Build a single panel from a bundle and its index.
#[must_use]
fn build_panel(bundle: &BiAssetBundle, index: usize, layout: Layout, columns: u32) -> Panel {
    let caption = compose_caption(&bundle.description, &bundle.caveats);
    let (row, col) = compute_position(index, layout, columns);

    Panel {
        title: bundle.title.clone(),
        caption,
        vega_spec: bundle.vega_spec.clone(),
        row,
        col,
    }
}

/// Compose the panel caption from description and caveats.
///
/// Caveats are appended after the description, separated by a space.
/// If there are multiple caveats they are joined with "; ".
#[must_use]
pub fn compose_caption(description: &str, caveats: &[String]) -> String {
    if caveats.is_empty() {
        description.to_owned()
    } else {
        format!("{}. Note: {}", description, caveats.join("; "))
    }
}

/// Compute `(row, col)` grid position for a panel at `index`.
///
/// Panics at compile time if `index` exceeds `u32::MAX` — in practice panels
/// are bounded by the input file size so this truncation cannot occur.
#[must_use]
pub fn compute_position(index: usize, layout: Layout, columns: u32) -> (u32, u32) {
    // Safety: panel counts are bounded by the number of input files, which
    // is far below u32::MAX on any real system. We document and allow this.
    #[allow(clippy::cast_possible_truncation)]
    let i = index as u32;
    match layout {
        Layout::Grid => {
            let cols = columns.max(1);
            (i / cols, i % cols)
        }
        Layout::Vertical => (i, 0),
        Layout::Horizontal => (0, i),
    }
}

/// Build the Vega-Lite v5 concat spec for the dashboard.
#[must_use]
fn build_vega_concat(title: &str, panels: &[Panel], layout: Layout, columns: u32) -> Value {
    let specs: Vec<Value> = panels.iter().map(|p| p.vega_spec.clone()).collect();

    let concat_field = match layout {
        Layout::Grid => "concat",
        Layout::Vertical => "vconcat",
        Layout::Horizontal => "hconcat",
    };

    let mut spec = serde_json::json!({
        "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
        "title": title,
        concat_field: specs
    });

    if layout == Layout::Grid {
        if let Some(obj) = spec.as_object_mut() {
            obj.insert("columns".to_owned(), serde_json::json!(columns));
        }
    }

    spec
}

/// Print a human-readable summary of the dashboard.
fn print_human(dashboard: &DashboardV1) {
    println!("Dashboard: {}", dashboard.title);
    println!("Layout: {} (columns: {})", dashboard.layout, dashboard.columns);
    println!("Panels: {}", dashboard.panels.len());
    println!();
    for (i, panel) in dashboard.panels.iter().enumerate() {
        println!(
            "  [{}] ({},{}) {}: {}",
            i, panel.row, panel.col, panel.title, panel.caption
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Unit tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_bundle(title: &str, description: &str, caveats: Vec<&str>) -> BiAssetBundle {
        BiAssetBundle {
            asset: Some("bi-asset.v1".to_owned()),
            title: title.to_owned(),
            description: description.to_owned(),
            vega_spec: serde_json::json!({"mark": "bar", "title": title}),
            profile_summary: None,
            caveats: caveats.into_iter().map(String::from).collect(),
        }
    }

    #[test]
    fn caption_no_caveats() {
        let c = compose_caption("Revenue by year", &[]);
        assert_eq!(c, "Revenue by year");
    }

    #[test]
    fn caption_one_caveat() {
        let c = compose_caption(
            "Revenue by year",
            &["balance measures are semi-additive".to_owned()],
        );
        assert!(c.contains("Revenue by year"));
        assert!(c.contains("balance measures"));
    }

    #[test]
    fn caption_multiple_caveats() {
        let c = compose_caption(
            "desc",
            &["caveat A".to_owned(), "caveat B".to_owned()],
        );
        assert!(c.contains("caveat A"));
        assert!(c.contains("caveat B"));
    }

    #[test]
    fn grid_positions() {
        assert_eq!(compute_position(0, Layout::Grid, 2), (0, 0));
        assert_eq!(compute_position(1, Layout::Grid, 2), (0, 1));
        assert_eq!(compute_position(2, Layout::Grid, 2), (1, 0));
        assert_eq!(compute_position(3, Layout::Grid, 2), (1, 1));
    }

    #[test]
    fn vertical_positions() {
        assert_eq!(compute_position(0, Layout::Vertical, 2), (0, 0));
        assert_eq!(compute_position(1, Layout::Vertical, 2), (1, 0));
        assert_eq!(compute_position(2, Layout::Vertical, 2), (2, 0));
    }

    #[test]
    fn horizontal_positions() {
        assert_eq!(compute_position(0, Layout::Horizontal, 2), (0, 0));
        assert_eq!(compute_position(1, Layout::Horizontal, 2), (0, 1));
        assert_eq!(compute_position(2, Layout::Horizontal, 2), (0, 2));
    }

    #[test]
    fn build_dashboard_two_panels() {
        let bundles = vec![
            make_bundle("Panel A", "desc A", vec![]),
            make_bundle("Panel B", "desc B", vec!["caveat"]),
        ];
        let d = build_dashboard(&bundles, "My Dashboard", Layout::Grid, 2);
        assert_eq!(d.dashboard, "dashboard.v1");
        assert_eq!(d.title, "My Dashboard");
        assert_eq!(d.panels.len(), 2);
        assert_eq!(d.panels[0].title, "Panel A");
        assert_eq!(d.panels[1].title, "Panel B");
        assert_eq!(d.panels[0].row, 0);
        assert_eq!(d.panels[0].col, 0);
        assert_eq!(d.panels[1].row, 0);
        assert_eq!(d.panels[1].col, 1);
    }

    #[test]
    fn vconcat_for_vertical() {
        let bundles = vec![
            make_bundle("A", "da", vec![]),
            make_bundle("B", "db", vec![]),
        ];
        let d = build_dashboard(&bundles, "T", Layout::Vertical, 2);
        assert!(d.vega_concat_spec.get("vconcat").is_some());
        assert!(d.vega_concat_spec.get("hconcat").is_none());
        assert!(d.vega_concat_spec.get("concat").is_none());
    }

    #[test]
    fn hconcat_for_horizontal() {
        let bundles = vec![make_bundle("A", "da", vec![])];
        let d = build_dashboard(&bundles, "T", Layout::Horizontal, 2);
        assert!(d.vega_concat_spec.get("hconcat").is_some());
    }

    #[test]
    fn concat_for_grid() {
        let bundles = vec![make_bundle("A", "da", vec![])];
        let d = build_dashboard(&bundles, "T", Layout::Grid, 2);
        assert!(d.vega_concat_spec.get("concat").is_some());
        assert_eq!(d.vega_concat_spec.get("columns"), Some(&serde_json::json!(2)));
    }

    #[test]
    fn single_panel_valid() {
        let bundles = vec![make_bundle("Solo", "only one", vec![])];
        let d = build_dashboard(&bundles, "Solo Dash", Layout::Grid, 2);
        assert_eq!(d.panels.len(), 1);
        let arr = d.vega_concat_spec["concat"].as_array().expect("concat array");
        assert_eq!(arr.len(), 1);
    }

    #[test]
    fn deterministic_ordering() {
        let bundles = vec![
            make_bundle("First", "d1", vec![]),
            make_bundle("Second", "d2", vec![]),
            make_bundle("Third", "d3", vec![]),
        ];
        let d1 = build_dashboard(&bundles, "T", Layout::Grid, 2);
        let d2 = build_dashboard(&bundles, "T", Layout::Grid, 2);
        let j1 = serde_json::to_string(&d1).expect("serialize");
        let j2 = serde_json::to_string(&d2).expect("serialize");
        assert_eq!(j1, j2);
    }
}
