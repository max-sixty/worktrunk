use clap::Subcommand;

/// Subcommands for `wt list`
#[derive(Subcommand)]
pub enum ListSubcommand {
    /// Single-line status for shell prompts
    #[command(after_long_help = "Output format: `branch  status  ±working  commits  upstream  ci`")]
    Statusline {
        /// Claude Code mode: read context from stdin, add directory and model
        ///
        /// Reads JSON from stdin with `.workspace.current_dir` and `.model.display_name`.
        /// Output: `dir  branch  status  ±working  commits  upstream  ci  | model`
        #[arg(long)]
        claude_code: bool,
    },
}
