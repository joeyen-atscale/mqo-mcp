//! `mqo-spec` binary — emits the JSON Schema for [`mqo_spec::Mqo`] to stdout.
//!
//! Usage:
//!   mqo-spec schema        — print the JSON Schema for Mqo
//!   mqo-spec validate <file> — validate a JSON file against the Mqo schema

use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let subcommand = args.get(1).map(String::as_str);

    match subcommand {
        Some("schema") => {
            println!("{}", mqo_spec::emit_json_schema());
        }
        Some("validate") => {
            let path = match args.get(2) {
                Some(p) => p,
                None => {
                    eprintln!("Usage: mqo-spec validate <file.json>");
                    process::exit(2);
                }
            };
            let raw = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error reading {path}: {e}");
                    process::exit(2);
                }
            };
            match serde_json::from_str::<mqo_spec::Mqo>(&raw) {
                Err(e) => {
                    eprintln!("Parse error: {e}");
                    process::exit(1);
                }
                Ok(mqo) => match mqo_spec::validate(&mqo) {
                    Ok(()) => {
                        println!("OK");
                    }
                    Err(errors) => {
                        for err in &errors {
                            eprintln!("Error: {err}");
                        }
                        process::exit(1);
                    }
                },
            }
        }
        _ => {
            eprintln!("Usage: mqo-spec <schema|validate>");
            eprintln!("  schema            — print JSON Schema for Mqo to stdout");
            eprintln!("  validate <file>   — parse and structurally validate a JSON file");
            process::exit(2);
        }
    }
}
