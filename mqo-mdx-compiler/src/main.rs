//! `mqo-mdx` — compile a `BoundMqo` JSON to an MDX SELECT query.
//!
//! Usage:
//!   mqo-mdx --bound `<bound_mqo.json>`
//!
//! Exit codes:
//!   0  — success; stdout is the MDX query string
//!   1  — compilation error (semi-additive missing trigger, empty measures, etc.)
//!   2  — I/O or JSON parse error

#![forbid(unsafe_code)]
#![deny(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]

use clap::Parser;
use mqo_mdx_compiler::{compile, validate_mdx_syntax, BoundMqoInput};
use std::path::PathBuf;
use std::process;

#[derive(Parser, Debug)]
#[command(
    name = "mqo-mdx",
    about = "Compile a BoundMqo JSON to an MDX SELECT query string"
)]
struct Args {
    /// Path to the `BoundMqo` JSON file (output of mqo-bind)
    #[arg(long)]
    bound: PathBuf,

    /// Skip the pre-flight MDX structural syntax check (exits 0 even when
    /// `validate_mdx_syntax` would reject the compiled output).
    #[arg(long, default_value_t = false)]
    skip_syntax_check: bool,
}

fn main() {
    let args = Args::parse();

    let text = std::fs::read_to_string(&args.bound).unwrap_or_else(|e| {
        eprintln!("mqo-mdx: cannot read --bound file: {e}");
        process::exit(2);
    });

    let bound: BoundMqoInput = serde_json::from_str(&text).unwrap_or_else(|e| {
        eprintln!("mqo-mdx: --bound file is not valid BoundMqo JSON: {e}");
        process::exit(2);
    });

    match compile(&bound) {
        Ok(mdx) => {
            if !args.skip_syntax_check {
                if let Err(e) = validate_mdx_syntax(&mdx) {
                    eprintln!("mqo-mdx: syntax check failed: {e}");
                    process::exit(1);
                }
            }
            println!("{mdx}");
            process::exit(0);
        }
        Err(e) => {
            eprintln!("mqo-mdx: compile error: {e}");
            process::exit(1);
        }
    }
}
