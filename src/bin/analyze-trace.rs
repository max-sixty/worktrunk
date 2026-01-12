//! Analyze wt-trace logs for concurrency visualization.
//!
//! # Usage
//!
//! ```bash
//! # Generate Chrome Trace Format for visualization
//! RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace > trace.json
//!
//! # Visualize: open trace.json in chrome://tracing or https://ui.perfetto.dev
//!
//! # Analyze with SQL (requires: curl -LO https://get.perfetto.dev/trace_processor)
//! trace_processor trace.json -Q 'SELECT COUNT(*), SUM(dur)/1e6 as cpu_ms FROM slice'
//! trace_processor trace.json -Q 'SELECT name, COUNT(*) as n, SUM(dur)/1e6 as ms FROM slice GROUP BY name ORDER BY ms DESC'
//!
//! # Find milestone events (instant events have dur=0)
//! trace_processor trace.json -Q 'SELECT name, ts/1e6 as ms FROM slice WHERE dur = 0'
//! ```

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use clap::Parser;
use worktrunk::trace;

/// Analyze wt-trace logs for concurrency visualization
#[derive(Parser)]
#[command(name = "analyze-trace")]
#[command(about = "Convert wt-trace logs to Chrome Trace Format for visualization")]
#[command(after_long_help = r#"EXAMPLES:
  # Generate trace from wt command
  RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace > trace.json

  # Then either:
  #   - Open trace.json in chrome://tracing or https://ui.perfetto.dev
  #   - Query with: trace_processor trace.json -Q 'SELECT * FROM slice LIMIT 10'

  # Find milestone events (instant events have dur=0)
  trace_processor trace.json -Q 'SELECT name, ts/1e6 as ms FROM slice WHERE dur = 0'

  # Install trace_processor for SQL analysis:
  curl -LO https://get.perfetto.dev/trace_processor && chmod +x trace_processor
"#)]
struct Args {
    /// Path to trace log file (reads from stdin if omitted)
    file: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();

    let input = match args.file {
        Some(path) if path.as_os_str() != "-" => match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(e) => {
                eprintln!("Error reading {}: {}", path.display(), e);
                std::process::exit(1);
            }
        },
        _ => {
            if std::io::stdin().is_terminal() {
                eprintln!("Reading from stdin... (pipe trace data or use Ctrl+D to end)");
                eprintln!();
                eprintln!("Hint: RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace");
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
        eprintln!("  [wt-trace] ts=1234567890 tid=3 cmd=\"git status\" dur=12.3ms ok=true");
        eprintln!("  [wt-trace] ts=1234567890 tid=3 event=\"Showed skeleton\"");
        eprintln!();
        eprintln!("To capture traces, run with RUST_LOG=debug:");
        eprintln!("  RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace");
        std::process::exit(1);
    }

    println!("{}", trace::to_chrome_trace(&entries));
}
