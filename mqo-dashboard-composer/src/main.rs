//! mqo-dashboard-composer — entry point.
//!
//! Parses CLI arguments and delegates to [`mqo_dashboard_composer::compose`].

use clap::Parser;
use mqo_dashboard_composer::{compose, ComposerArgs};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = ComposerArgs::parse();
    match compose(&args) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
