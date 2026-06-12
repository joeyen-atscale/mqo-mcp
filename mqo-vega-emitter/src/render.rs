//! Render-verification for emitted Vega-Lite v5 specs — `render` Cargo feature only.
//!
//! # WARNING: CI/release-gate use only
//!
//! This module is compiled only when the `render` Cargo feature is enabled.
//! The `render` feature requires the `vl-convert` CLI to be present on PATH at
//! runtime. `vl-convert` embeds the Vega/Vega-Lite JavaScript toolchain via a
//! V8 runtime and is large and slow to install. This module is intended for
//! **CI/release-gate** pipelines — not for inner-loop developer builds.
//!
//! Install `vl-convert` for the CI environment:
//! ```sh
//! pip install vl-convert-python   # installs the vl-convert CLI
//! # or: cargo install vl-convert (when stable)
//! ```
//!
//! # CI corpus render-gate invocation
//!
//! ```sh
//! # Run the render check over a corpus of specs (one JSON file per spec):
//! for f in specs/*.json; do
//!   cargo run --features render --bin mqo-vega-emitter -- \
//!     --recommendation "$f" --rows rows.json --render "${f%.json}.svg" \
//!     || { echo "FAIL: $f"; exit 1; }
//! done
//! ```

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use thiserror::Error;

/// Errors that can occur during render verification.
///
/// These are **separate** from [`crate::EmitError`]: a `RenderError` means the
/// spec was successfully emitted but the render step failed.  A caller can
/// therefore distinguish "spec structurally invalid" from "spec renderable."
#[derive(Debug, Error)]
pub enum RenderError {
    /// The `vl-convert` executable was not found on `PATH` (or at the configured path).
    ///
    /// Install with `pip install vl-convert-python` or `cargo install vl-convert`.
    #[error("`vl-convert` not found; install with `pip install vl-convert-python`")]
    ConverterNotFound,

    /// `vl-convert` exited non-zero.
    ///
    /// `stderr` contains the error output from the converter.
    #[error("`vl-convert` exited with error: {stderr}")]
    ConverterError {
        /// The stderr output from the converter.
        stderr: String,
    },

    /// The renderer produced empty output (zero bytes).
    ///
    /// A structurally-valid spec that produces no image bytes is considered a
    /// render failure per R6.
    #[error("render produced empty output for spec `{spec_id}`")]
    EmptyOutput {
        /// An identifier for the spec that failed to render (e.g. file path or label).
        spec_id: String,
    },

    /// An I/O error occurred while writing the rendered image or spawning the process.
    #[error("I/O error during render: {0}")]
    Io(#[from] std::io::Error),
}

/// Render output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderFormat {
    /// Scalable Vector Graphics — default when format is unambiguous.
    Svg,
    /// Portable Network Graphics — raster output.
    Png,
}

impl RenderFormat {
    /// Infer format from a file extension.  Returns `None` when the extension
    /// is unrecognised.
    #[must_use]
    pub fn from_path(p: &Path) -> Option<Self> {
        match p.extension()?.to_str()? {
            "svg" => Some(Self::Svg),
            "png" => Some(Self::Png),
            _ => None,
        }
    }

    const fn vl_convert_subcommand(self) -> &'static str {
        match self {
            Self::Svg => "vl2svg",
            Self::Png => "vl2png",
        }
    }
}

/// The name of the `vl-convert` executable that will be searched on PATH.
///
/// Overridable in tests via [`render_check_with_converter`].
const DEFAULT_CONVERTER: &str = "vl-convert";

/// Verify that `spec_json` can be rendered by `vl-convert` to a non-empty image.
///
/// Spawns `vl-convert <subcommand>` as a subprocess, pipes the spec on stdin,
/// and reads the image bytes from stdout.  Returns the image bytes on success.
///
/// The `spec_id` parameter is a human-readable label used in error messages
/// (e.g. the file path or the name of the spec under test).
///
/// # Errors
///
/// - [`RenderError::ConverterNotFound`] if `vl-convert` is absent from `PATH`.
/// - [`RenderError::ConverterError`] if `vl-convert` exits non-zero.
/// - [`RenderError::EmptyOutput`] if the output is zero bytes.
/// - [`RenderError::Io`] if spawning or I/O fails.
pub fn render_check(spec_json: &str, spec_id: &str, format: RenderFormat) -> Result<Vec<u8>, RenderError> {
    render_check_with_converter(spec_json, spec_id, format, DEFAULT_CONVERTER)
}

/// Like [`render_check`] but uses a caller-supplied converter executable name or path.
///
/// This variant is primarily for testing — callers can pass a known-absent
/// executable name (e.g. `"vl-convert-does-not-exist"`) to exercise error paths
/// without mutating process environment.
///
/// # Errors
///
/// Same as [`render_check`].
pub fn render_check_with_converter(
    spec_json: &str,
    spec_id: &str,
    format: RenderFormat,
    converter: &str,
) -> Result<Vec<u8>, RenderError> {
    let subcommand = format.vl_convert_subcommand();

    let mut child = Command::new(converter)
        .arg(subcommand)
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                RenderError::ConverterNotFound
            } else {
                RenderError::Io(e)
            }
        })?;

    // Write spec to stdin then close it (EOF signals end-of-input to the child).
    {
        let stdin = child.stdin.as_mut().ok_or_else(|| {
            RenderError::Io(std::io::Error::other("failed to open vl-convert stdin"))
        })?;
        stdin.write_all(spec_json.as_bytes()).map_err(RenderError::Io)?;
        // stdin is dropped here, sending EOF to the child.
    }

    let output = child.wait_with_output().map_err(RenderError::Io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(RenderError::ConverterError { stderr });
    }

    if output.stdout.is_empty() {
        return Err(RenderError::EmptyOutput {
            spec_id: spec_id.to_owned(),
        });
    }

    Ok(output.stdout)
}

/// Run a corpus render-gate across a slice of `(spec_id, spec_json)` pairs.
///
/// Returns `Ok(())` when all specs render successfully.  Returns the first
/// failure encountered as `Err`, with the `spec_id` of the failing spec and
/// the [`RenderError`] describing why it failed.  Satisfies R10–R11: every
/// failing spec is named, and the gate exits with an error when any fails.
///
/// # Errors
///
/// Returns the first `(spec_id, RenderError)` encountered.
pub fn corpus_render_gate<'a>(
    specs: &'a [(&'a str, &'a str)],
    format: RenderFormat,
) -> Result<(), (&'a str, RenderError)> {
    corpus_render_gate_with_converter(specs, format, DEFAULT_CONVERTER)
}

/// Like [`corpus_render_gate`] but uses a caller-supplied converter name.
///
/// Used in tests to exercise error paths without env mutation.
///
/// # Errors
///
/// Returns the first `(spec_id, RenderError)` encountered.
pub fn corpus_render_gate_with_converter<'a>(
    specs: &'a [(&'a str, &'a str)],
    format: RenderFormat,
    converter: &str,
) -> Result<(), (&'a str, RenderError)> {
    for (spec_id, spec_json) in specs {
        render_check_with_converter(spec_json, spec_id, format, converter)
            .map_err(|e| (*spec_id, e))?;
    }
    Ok(())
}
