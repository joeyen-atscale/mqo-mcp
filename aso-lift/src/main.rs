//! `aso-lift` CLI — lift an engine model XML file to RDF (Turtle).
//!
//! Usage:
//!   aso-lift <model.xml> [--base-iri <IRI>] [--output <file.ttl>]
//!
//! Exits 0 on success, 1 on error.

use std::path::PathBuf;
use std::process;

fn usage() -> ! {
    eprintln!(
        "usage: aso-lift <model.xml> [--base-iri <IRI>] [--output <file.ttl>]\n\
         \n\
         Lifts an AtScale engine model XML (project_2_0 schema) to RDF (Turtle).\n\
         Writes to <file.ttl> if --output is given, otherwise to stdout.\n\
         \n\
         NFR2: This tool never connects to a data warehouse. It reads metadata XML only."
    );
    process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
    }

    let mut xml_path: Option<PathBuf> = None;
    let mut base_iri = aso_lift::LiftOptions::default().base_iri;
    let mut output_path: Option<PathBuf> = None;

    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--base-iri" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --base-iri requires an argument");
                    process::exit(1);
                }
                base_iri = args[i].clone();
            }
            "--output" | "-o" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: --output requires an argument");
                    process::exit(1);
                }
                output_path = Some(PathBuf::from(&args[i]));
            }
            arg if !arg.starts_with('-') => {
                xml_path = Some(PathBuf::from(arg));
            }
            other => {
                eprintln!("error: unknown argument '{other}'");
                usage();
            }
        }
        i += 1;
    }

    let xml_path = xml_path.unwrap_or_else(|| usage());

    let xml = match std::fs::read_to_string(&xml_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read '{}': {e}", xml_path.display());
            process::exit(1);
        }
    };

    let opts = aso_lift::LiftOptions { base_iri };

    match aso_lift::lift(&xml, &opts) {
        Ok(output) => {
            eprintln!(
                "[aso-lift] lifted {} triples from '{}'",
                output.triple_count,
                xml_path.display()
            );
            match output_path {
                Some(ref path) => {
                    if let Err(e) = std::fs::write(path, &output.turtle) {
                        eprintln!("error: could not write '{}': {e}", path.display());
                        process::exit(1);
                    }
                    eprintln!("[aso-lift] wrote Turtle to '{}'", path.display());
                }
                None => {
                    print!("{}", output.turtle);
                }
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    }
}
