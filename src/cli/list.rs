use clap::Subcommand;

use super::StatuslineFormat;

/// Subcommands for `wt list`
#[derive(Subcommand)]
pub enum ListSubcommand {
    /// Single-line status for the current worktree
    #[command(
        after_long_help = r#"The line carries the same cells as the worktree's row in `wt list`. A stale CI status cache makes it reach the network for a second or two, so it fits a statusline the host renders in the background — Claude Code's, a `tmux` status bar — better than a prompt the shell blocks on. Want it fast enough for a synchronous prompt? Open an issue at https://github.com/max-sixty/worktrunk.

## Output formats

- `table` (default): `branch  status  HEAD±  main↕  main…±  Remote⇅  CI  URL`
- `json`: the current [`wt list --format=json`](/list/#json-output) schema — a one-item envelope by default, or a one-item array with `[list] json-schema = 1`
- `claude-code`: the `table` cells, preceded by `dir` and followed by `model  context  pace`

A cell with nothing to show is left out, so most lines are shorter than that. A line that overruns the terminal drops whole cells, least important first.

The CI reference links to its PR/MR, and a dev server URL carrying a port shows as `:3000` linking to the URL in full, dim until something answers on that port. Both are underlined terminal links.

## Claude Code mode

`--format=claude-code` reads JSON context from stdin (`.workspace.current_dir` is required; the rest are optional):

- `.workspace.current_dir` — working directory
- `.model.display_name` — model name
- `.context_window.used_percentage` — context usage (0–100), rendered as `🌔 65%`, the moon waning 🌕→🌑 as context fills
- `.rate_limits.{five_hour,seven_day}.used_percentage` — rate-limit window usage (0–100)
- `.rate_limits.{five_hour,seven_day}.resets_at` — window reset time (Unix epoch seconds)

The pace segment appears only when usage is likely to hit a rate limit before its window resets, and shows the higher-risk window: `2.9×(Tue–Tue 5pm)` reads as 2.9× the pace that would exactly fill that window. Above 90% used it shows usage instead of pace — `93%(Tue–Tue 5pm)`. Its color deepens from dim to yellow as more of the window would be spent capped.

[Claude Code statusline setup](/claude-code/#statusline-claude-code-only) has the `~/.claude/settings.json` entry that feeds this mode.
"#
    )]
    Statusline {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: StatuslineFormat,

        /// Deprecated: use --format=claude-code
        #[arg(long, hide = true)]
        claude_code: bool,
    },
}
