//! Analyze wt-trace logs to understand command performance.
//!
//! # Usage
//!
//! ```bash
//! # Human-readable analysis
//! RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace
//!
//! # Chrome Trace Format for visualization or SQL analysis
//! RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace --format=chrome > trace.json
//!
//! # Visualize: open trace.json in chrome://tracing or https://ui.perfetto.dev
//!
//! # Analyze with SQL (requires: curl -LO https://get.perfetto.dev/trace_processor)
//! trace_processor trace.json -Q 'SELECT COUNT(*), SUM(dur)/1e6 as cpu_ms FROM slice'
//! ```

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use worktrunk::trace;

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum OutputFormat {
    /// Human-readable analysis with command breakdown and histogram
    #[default]
    Text,
    /// Chrome Trace Format JSON for chrome://tracing, Perfetto, or trace_processor
    Chrome,
}

/// Analyze wt-trace logs for performance insights
#[derive(Parser)]
#[command(name = "analyze-trace")]
#[command(about = "Analyze wt-trace logs for performance insights")]
#[command(after_long_help = r#"EXAMPLES:
  # Human-readable analysis
  RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace

  # Chrome Trace Format for visualization
  RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace --format=chrome > trace.json

  # Then either:
  #   - Open trace.json in chrome://tracing or https://ui.perfetto.dev
  #   - Query with: trace_processor trace.json -Q 'SELECT * FROM slice LIMIT 10'

  # Install trace_processor for SQL analysis:
  curl -LO https://get.perfetto.dev/trace_processor && chmod +x trace_processor
"#)]
struct Args {
    /// Path to trace log file (reads from stdin if omitted)
    file: Option<PathBuf>,

    /// Output format
    #[arg(long, short, value_enum, default_value_t = OutputFormat::Text)]
    format: OutputFormat,
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
        eprintln!();
        eprintln!("To capture traces, run with RUST_LOG=debug:");
        eprintln!("  RUST_LOG=debug wt list 2>&1 | grep wt-trace | analyze-trace");
        std::process::exit(1);
    }

    match args.format {
        OutputFormat::Text => {
            let analysis = trace::analyze(&entries);
            println!("{}", trace::render(&analysis));
        }
        OutputFormat::Chrome => {
            println!("{}", trace::to_chrome_trace(&entries));
        }
    }
}
