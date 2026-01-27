use clap::Subcommand;

use super::OutputFormat;

/// Subcommands for `wt list`
#[derive(Subcommand)]
pub enum ListSubcommand {
    /// Single-line status for shell prompts
    #[command(after_long_help = r#"## Output formats

- `table` (default): `branch  status  ±working  commits  upstream  ci`
- `json`: Same structure as `wt list --format=json` but for the current worktree only
- `claude-code`: Reads context from stdin, adds directory and model segments

## Claude Code mode

With `--format=claude-code`, reads JSON context from stdin:
`dir  branch  status  ±working  commits  upstream  ci  | model  context`

Input fields (all optional):
- `.workspace.current_dir` — working directory
- `.model.display_name` — model name
- `.context_window.used_percentage` — context usage (0-100)
"#)]
    Statusline {
        /// Output format (table, json, claude-code)
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,

        /// Deprecated: use --format=claude-code
        #[arg(long, hide = true)]
        claude_code: bool,
    },
}
