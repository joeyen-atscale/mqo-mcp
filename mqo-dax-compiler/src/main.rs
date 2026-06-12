//! `mqo-dax` — compile a `BoundMqo` JSON to a DAX `EVALUATE` statement.
//!
//! Usage:
//!   `mqo-dax --bound <bound_mqo.json>`
//!
//! Stdout: DAX text.
//! Exit codes:
//!   0 — success
//!   1 — compile error
//!   2 — bad arguments or I/O error

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use clap::Parser;
use mqo_dax_compiler::{
    compile, compile_grounded,
    catalog_context::DaxCatalogContext,
    input::BoundMqoInput,
    syntax_check::validate_dax_syntax,
};
use std::path::PathBuf;
use std::process;

#[derive(Parser, Debug)]
#[command(
    name = "mqo-dax",
    about = "Compile a BoundMqo JSON to a DAX EVALUATE statement"
)]
struct Args {
    /// Path to the `BoundMqo` JSON file produced by `mqo-bind`.
    #[arg(long)]
    bound: PathBuf,

    /// Path to a `CatalogSnapshot` JSON file.  When provided, column references
    /// are grounded to engine-ready `'TableName'[Display Label]` / `[Display Label]` forms.
    #[arg(long)]
    catalog: Option<PathBuf>,

    /// Skip the bundled syntax check (not recommended).
    #[arg(long, default_value_t = false)]
    skip_syntax_check: bool,
}

fn main() {
    let args = Args::parse();

    let text = std::fs::read_to_string(&args.bound).unwrap_or_else(|e| {
        eprintln!("mqo-dax: cannot read --bound file: {e}");
        process::exit(2);
    });

    let bound: BoundMqoInput = serde_json::from_str(&text).unwrap_or_else(|e| {
        eprintln!("mqo-dax: --bound file is not valid BoundMqo JSON: {e}");
        process::exit(2);
    });

    // Load the optional catalog context.
    let catalog_ctx: Option<DaxCatalogContext> = match &args.catalog {
        None => None,
        Some(path) => {
            let catalog_text = std::fs::read_to_string(path).unwrap_or_else(|e| {
                eprintln!("mqo-dax: cannot read --catalog file: {e}");
                process::exit(2);
            });
            let ctx = DaxCatalogContext::from_json(&catalog_text).unwrap_or_else(|e| {
                eprintln!("mqo-dax: --catalog file is not valid CatalogSnapshot JSON: {e}");
                process::exit(2);
            });
            Some(ctx)
        }
    };

    let dax = match catalog_ctx {
        Some(ref ctx) => compile_grounded(&bound, Some(ctx)),
        None => compile(&bound),
    };

    let dax = match dax {
        Ok(d) => d,
        Err(e) => {
            eprintln!("mqo-dax: compile error: {e}");
            process::exit(1);
        }
    };

    if !args.skip_syntax_check {
        if let Err(e) = validate_dax_syntax(&dax) {
            eprintln!("mqo-dax: syntax check failed: {e}");
            process::exit(1);
        }
    }

    println!("{dax}");
}
