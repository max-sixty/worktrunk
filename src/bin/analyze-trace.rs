//! Analyze wt-trace logs to understand command performance.
//!
//! # Usage
//!
//! ```bash
//! # Analyze from file
//! analyze-trace /path/to/trace.log
//!
//! # Analyze from stdin
//! RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace
//!
//! # Capture and analyze
//! RUST_LOG=debug wt list --branches 2>&1 | grep wt-trace > trace.log
//! analyze-trace trace.log
//! ```

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use worktrunk::trace;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let input = match args.get(1) {
        Some(path) if path != "-" => {
            // Read from file
            let path = PathBuf::from(path);
            match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(e) => {
                    eprintln!("Error reading {}: {}", path.display(), e);
                    std::process::exit(1);
                }
            }
        }
        _ => {
            // Read from stdin
            if std::io::stdin().is_terminal() {
                eprintln!("Usage: analyze-trace <file> | analyze-trace < input");
                eprintln!();
                eprintln!("Examples:");
                eprintln!("  analyze-trace /tmp/trace.log");
                eprintln!("  RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace");
                std::process::exit(1);
            }

            let mut content = String::new();
            std::io::stdin()
                .lock()
                .read_to_string(&mut content)
                .expect("Failed to read stdin");
            content
        }
    };

    let entries = trace::parse_lines(&input);

    if entries.is_empty() {
        eprintln!("No trace entries found in input.");
        eprintln!();
        eprintln!("Trace lines should look like:");
        eprintln!("  [wt-trace] cmd=\"git status\" dur=12.3ms ok=true");
        eprintln!();
        eprintln!("To capture traces, run with RUST_LOG=debug:");
        eprintln!("  RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace");
        std::process::exit(1);
    }

    let analysis = trace::analyze(&entries);
    println!("{}", trace::render(&analysis));
}
