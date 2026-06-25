use clap::Subcommand;

use super::StatuslineFormat;

/// Subcommands for `wt list`
#[derive(Subcommand)]
pub enum ListSubcommand {
    /// Single-line status for shell prompts
    #[command(after_long_help = r#"## Output formats

- `table` (default): `branch  status  ±working  commits  upstream  ci`
- `json`: Same structure as `wt list --format=json` but for the current worktree only
- `claude-code`: `dir  branch  status  ±working  commits  upstream  ci  model  context  pace`

## Claude Code mode

`--format=claude-code` reads JSON context from stdin (`.workspace.current_dir` is required; the rest are optional):

- `.workspace.current_dir` — working directory
- `.model.display_name` — model name
- `.context_window.used_percentage` — context usage (0–100)
- `.rate_limits.{five_hour,seven_day}.used_percentage` — rate-limit window usage (0–100)
- `.rate_limits.{five_hour,seven_day}.resets_at` — window reset time (Unix epoch seconds)

The pace segment appears only when usage is likely to hit a rate limit before its window resets, and shows the higher-risk window: `2.9×(Tue–Tue 5pm)` reads as 2.9× the pace that would exactly fill that window. Above 90% used it shows usage instead of pace — `93%(Tue–Tue 5pm)` — near the cap, how much is left matters more than how fast it's going. "Likely" is a Bayesian forecast; early-window bursts don't trigger it. Its colour deepens with severity — dim, then dim-yellow, then yellow — as the forecast lockout (how much of the window would be spent capped) grows, so a fast pace that would only tip over near the reset stays dim rather than alarming. With `-vv`, each window's inputs and projection are logged to `.git/wt/logs/trace.log`.
"#)]
    Statusline {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: StatuslineFormat,

        /// Deprecated: use --format=claude-code
        #[arg(long, hide = true)]
        claude_code: bool,
    },
}
