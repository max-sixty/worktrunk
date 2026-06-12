use clap::Subcommand;

use super::OutputFormat;

/// Subcommands for `wt list`
#[derive(Subcommand)]
pub enum ListSubcommand {
    /// Single-line status for shell prompts
    #[command(after_long_help = r#"## Output formats

- `table` (default): `branch  status  ±working  commits  upstream  ci`
- `json`: Same structure as `wt list --format=json` but for the current worktree only
- `claude-code`: Reads context from stdin, adds directory, model, context, and rate-limit segments

## Claude Code mode

With `--format=claude-code`, reads JSON context from stdin:
`dir  branch  status  ±working  commits  upstream  ci  model  context  pace`

Input fields (`.workspace.current_dir` is required; the rest are optional):
- `.workspace.current_dir` — working directory
- `.model.display_name` — model name
- `.context_window.used_percentage` — context usage (0-100)
- `.rate_limits.{five_hour,seven_day}.used_percentage` — rate-limit window usage (0-100)
- `.rate_limits.{five_hour,seven_day}.resets_at` — window reset time (Unix epoch seconds)

The pace segment (e.g. `2.9×pace(Tue–Tue 5pm)`) appears only when usage is
likely to hit a rate limit before its window resets, and shows the
higher-risk window. With `-vv`, each window's inputs and projection are
logged to `.git/wt/logs/trace.log`.
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
