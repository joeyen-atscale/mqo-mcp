//! mqo-vega-emitter — CLI: read recommendation + rows from files, emit VL5 spec.
//!
//! Usage: `mqo-vega-emitter --recommendation <f> --rows <f> [--pretty]`
//!
//! When built with `--features render`:
//!   `mqo-vega-emitter --recommendation <f> --rows <f> --render <out.svg|out.png>`

// The binary's sole purpose is I/O — printing to stdout/stderr is intentional.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

/// Parsed CLI arguments.
struct Args {
    recommendation: PathBuf,
    rows: PathBuf,
    pretty: bool,
    /// Output path for render-verification (only used with the `render` feature).
    #[cfg_attr(not(feature = "render"), allow(dead_code))]
    render_out: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let raw: Vec<String> = std::env::args().collect();
    let mut recommendation: Option<PathBuf> = None;
    let mut rows: Option<PathBuf> = None;
    let mut pretty = false;
    #[cfg(feature = "render")]
    let mut render_out: Option<PathBuf> = None;
    #[cfg(not(feature = "render"))]
    let render_out: Option<PathBuf> = None;

    let mut i = 1usize;
    while i < raw.len() {
        let arg = raw.get(i).map_or("", String::as_str);
        match arg {
            "--recommendation" => {
                i += 1;
                recommendation = raw.get(i).map(PathBuf::from);
            }
            "--rows" => {
                i += 1;
                rows = raw.get(i).map(PathBuf::from);
            }
            "--pretty" => {
                pretty = true;
            }
            "--render" => {
                i += 1;
                let out = raw.get(i).map(PathBuf::from).ok_or_else(|| {
                    "--render requires an output path argument".to_owned()
                })?;
                #[cfg(not(feature = "render"))]
                {
                    return Err(format!(
                        "--render is not available in this build; recompile with `--features render` (got path: {})",
                        out.display()
                    ));
                }
                #[cfg(feature = "render")]
                {
                    render_out = Some(out);
                }
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                return Err(format!("unknown argument: {other}"));
            }
        }
        i += 1;
    }

    let recommendation =
        recommendation.ok_or_else(|| "--recommendation <path> is required".to_owned())?;
    let rows = rows.ok_or_else(|| "--rows <path> is required".to_owned())?;

    Ok(Args {
        recommendation,
        rows,
        pretty,
        render_out,
    })
}

fn print_help() {
    eprintln!("mqo-vega-emitter");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("  mqo-vega-emitter --recommendation <path> --rows <path> [--pretty]");
    #[cfg(feature = "render")]
    eprintln!("  mqo-vega-emitter --recommendation <path> --rows <path> --render <out.svg|out.png>");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("  --recommendation <path>  Path to chart-recommendation.v1 JSON file");
    eprintln!("  --rows <path>            Path to rows JSON array file");
    eprintln!("  --pretty                 Pretty-print the output JSON");
    #[cfg(feature = "render")]
    {
        eprintln!("  --render <path>          Render-verify: emit spec, render to SVG/PNG, write");
        eprintln!("                           image to <path>; exit non-zero on render failure.");
        eprintln!("                           Format is inferred from file extension (.svg/.png).");
        eprintln!("                           Requires `vl-convert` on PATH.");
    }
    eprintln!("  --help, -h               Print this help message");
}

fn run() -> Result<(), String> {
    let args = parse_args()?;

    let rec_text = fs::read_to_string(&args.recommendation).map_err(|e| {
        format!(
            "failed to read recommendation file `{}`: {e}",
            args.recommendation.display()
        )
    })?;
    let rows_text = fs::read_to_string(&args.rows).map_err(|e| {
        format!(
            "failed to read rows file `{}`: {e}",
            args.rows.display()
        )
    })?;

    let recommendation: serde_json::Value =
        serde_json::from_str(&rec_text).map_err(|e| format!("invalid recommendation JSON: {e}"))?;
    let rows_val: serde_json::Value =
        serde_json::from_str(&rows_text).map_err(|e| format!("invalid rows JSON: {e}"))?;

    let rows = rows_val
        .as_array()
        .ok_or_else(|| "rows must be a JSON array".to_owned())?;

    let spec =
        mqo_vega_emitter::emit(&recommendation, rows).map_err(|e| format!("emit error: {e}"))?;

    #[cfg(feature = "render")]
    if let Some(out_path) = &args.render_out {
        return run_render(&spec, out_path);
    }

    let output = if args.pretty {
        serde_json::to_string_pretty(&spec).map_err(|e| format!("serialization error: {e}"))?
    } else {
        serde_json::to_string(&spec).map_err(|e| format!("serialization error: {e}"))?
    };

    println!("{output}");
    Ok(())
}

#[cfg(feature = "render")]
fn run_render(spec: &serde_json::Value, out_path: &std::path::Path) -> Result<(), String> {
    use mqo_vega_emitter::render::{render_check, RenderFormat};

    let format = RenderFormat::from_path(out_path).unwrap_or(RenderFormat::Svg);
    let spec_json =
        serde_json::to_string(spec).map_err(|e| format!("serialization error: {e}"))?;
    let spec_id = out_path.display().to_string();

    let image_bytes = render_check(&spec_json, &spec_id, format)
        .map_err(|e| format!("render error: {e}"))?;

    fs::write(out_path, &image_bytes)
        .map_err(|e| format!("failed to write render output to `{}`: {e}", out_path.display()))?;

    eprintln!("render: wrote {} bytes to `{}`", image_bytes.len(), out_path.display());
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
