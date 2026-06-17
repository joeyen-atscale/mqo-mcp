//! `aso-ground` CLI — ground a lifted Turtle graph against BFO 2020.
//!
//! ## Usage
//!
//! ```text
//! aso-ground <lifted.ttl>               # emit overlay to stdout
//! aso-ground <lifted.ttl> --output <f>  # emit overlay to file
//! aso-ground <lifted.ttl> report        # print coverage report only
//! ```

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

/// Deterministic kind-driven BFO 2020 grounding overlay over a lifted aso: graph.
#[derive(Parser, Debug)]
#[command(name = "aso-ground", version, about)]
struct Cli {
    /// Path to the lifted Turtle file (output of aso-lift).
    input: PathBuf,

    /// Output path for the overlay Turtle (defaults to stdout).
    #[arg(short, long)]
    output: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Print a coverage report and exit (no overlay emitted).
    Report,
}

fn main() {
    let cli = Cli::parse();

    // Read input
    let turtle_input = match std::fs::read_to_string(&cli.input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("aso-ground: error reading '{}': {e}", cli.input.display());
            process::exit(1);
        }
    };

    // Ground
    let grounded = match aso_ground::ground(&turtle_input) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("aso-ground: grounding error: {e}");
            process::exit(2);
        }
    };

    // Dispatch subcommand
    match cli.command {
        Some(Commands::Report) => {
            let rpt = aso_ground::report(&grounded);
            println!("{}", rpt.display());
        }
        None => {
            // Emit overlay
            let overlay = match aso_ground::emit_overlay(&grounded) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("aso-ground: overlay emission error: {e}");
                    process::exit(3);
                }
            };

            match cli.output {
                Some(path) => {
                    if let Err(e) = std::fs::write(&path, &overlay) {
                        eprintln!("aso-ground: error writing '{}': {e}", path.display());
                        process::exit(4);
                    }
                }
                None => {
                    print!("{overlay}");
                }
            }

            // Always print the report to stderr
            let rpt = aso_ground::report(&grounded);
            eprintln!("{}", rpt.display());
        }
    }
}
