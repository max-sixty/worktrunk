mod config;
mod hook;
mod list;
mod step;

pub(crate) use config::{
    ApprovalsCommand, CiStatusAction, ConfigAliasCommand, ConfigCommand,
    ConfigPluginsClaudeCommand, ConfigPluginsCommand, ConfigPluginsOpencodeCommand,
    ConfigShellCommand, DefaultBranchAction, HintsAction, LogsAction, MarkerAction,
    PreviousBranchAction, StateCommand, VarsAction,
};
pub(crate) use hook::{HOOK_TYPE_NAMES, HookCommand, HookOptions};
pub(crate) use list::ListSubcommand;
pub(crate) use step::StepCommand;

use clap::builder::styling::{AnsiColor, Color, Styles};
use clap::{Args, Command, CommandFactory, Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::sync::OnceLock;

use crate::commands::Shell;

/// Parse KEY=VALUE string for `wt config state vars set`.
///
/// Like `parse_key_val`, but without hyphen→underscore canonicalization.
/// Key validation is deferred to `validate_vars_key` in the command handler.
pub(super) fn parse_vars_assignment(s: &str) -> Result<(String, String), String> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| format!("invalid KEY=VALUE: no `=` found in `{s}`"))?;
    if key.is_empty() {
        return Err("invalid KEY=VALUE: key cannot be empty".to_string());
    }
    Ok((key.to_string(), value.to_string()))
}

/// Custom styles for help output - matches worktrunk's color scheme
pub(crate) fn help_styles() -> Styles {
    Styles::styled()
        .header(
            anstyle::Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Green))),
        )
        .usage(
            anstyle::Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Green))),
        )
        .literal(
            anstyle::Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Cyan))),
        )
        .placeholder(anstyle::Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan))))
        .error(
            anstyle::Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Red))),
        )
        .valid(
            anstyle::Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Green))),
        )
        .invalid(
            anstyle::Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Yellow))),
        )
}

/// Default command name for worktrunk
const DEFAULT_COMMAND_NAME: &str = "wt";

/// Help template for commands
const HELP_TEMPLATE: &str = "\
{before-help}{name} - {about-with-newline}
Usage: {usage}

{all-args}{after-help}";

/// Cached value_name for Shell enum (e.g., "bash|fish|zsh|powershell")
///
/// TODO: There should be a simpler way to show ValueEnum variants in clap's "missing required
/// argument" error. Clap auto-generates `[possible values: ...]` in help and completions from
/// ValueEnum, but doesn't use it for value_name. We use mut_subcommand to set it dynamically,
/// but this feels overly complex. Revisit if clap adds better support.
fn shell_value_name() -> &'static str {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            Shell::value_variants()
                .iter()
                .filter_map(|v| v.to_possible_value())
                .map(|v| v.get_name().to_owned())
                .collect::<Vec<_>>()
                .join("|")
        })
        .as_str()
}

/// Build a clap Command for Cli with the shared help template applied recursively.
pub(crate) fn build_command() -> Command {
    let cmd = apply_help_template_recursive(Cli::command(), DEFAULT_COMMAND_NAME);

    // Set value_name for Shell args to show options in usage/errors
    let shell_name = shell_value_name();
    cmd.mut_subcommand("config", |c| {
        c.mut_subcommand("shell", |c| {
            c.mut_subcommand("init", |c| c.mut_arg("shell", |a| a.value_name(shell_name)))
                .mut_subcommand("install", |c| {
                    c.mut_arg("shell", |a| a.value_name(shell_name))
                })
                .mut_subcommand("uninstall", |c| {
                    c.mut_arg("shell", |a| a.value_name(shell_name))
                })
        })
    })
}

/// Parent commands whose subcommands can be suggested for unrecognized top-level commands.
const NESTED_COMMAND_PARENTS: &[&str] = &["step", "hook"];

/// Check if an unrecognized subcommand matches a nested subcommand.
///
/// Returns the full command path if found, e.g., "wt step squash" for "squash".
pub(crate) fn suggest_nested_subcommand(cmd: &Command, unknown: &str) -> Option<String> {
    for parent in NESTED_COMMAND_PARENTS {
        if let Some(parent_cmd) = cmd.get_subcommands().find(|c| c.get_name() == *parent)
            && parent_cmd
                .get_subcommands()
                .any(|s| s.get_name() == unknown)
        {
            return Some(format!("wt {parent} {unknown}"));
        }
    }
    // Hook types aren't clap subcommands of `hook` (they're caught by
    // `external_subcommand`), so the structural search above misses them.
    // Check the canonical name list directly so `wt pre-merge` → `wt hook
    // pre-merge` still suggests correctly.
    if HOOK_TYPE_NAMES.contains(&unknown) {
        return Some(format!("wt hook {unknown}"));
    }
    None
}

fn apply_help_template_recursive(mut cmd: Command, path: &str) -> Command {
    cmd = cmd.help_template(HELP_TEMPLATE).display_name(path);

    for sub in cmd.get_subcommands_mut() {
        let sub_cmd = std::mem::take(sub);
        let sub_path = format!("{} {}", path, sub_cmd.get_name());
        let sub_cmd = apply_help_template_recursive(sub_cmd, &sub_path);
        *sub = sub_cmd;
    }
    cmd
}

/// Get the version string for display.
///
/// Returns the git describe version if available (e.g., "v0.8.5-3-gabcdef"),
/// otherwise falls back to the cargo package version (e.g., "0.8.5").
pub(crate) fn version_str() -> &'static str {
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION.get_or_init(|| {
        let git_version = env!("VERGEN_GIT_DESCRIBE");
        let cargo_version = env!("CARGO_PKG_VERSION");

        if git_version.contains("IDEMPOTENT") {
            cargo_version.to_string()
        } else {
            git_version.to_string()
        }
    })
}

/// Output format for commands with text + JSON modes (e.g., `wt switch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum SwitchFormat {
    /// Human-readable text output
    Text,
    /// JSON output
    Json,
}

// TODO: ClaudeCode is statusline-specific but lives in this shared enum, forcing
// unrelated codepaths to handle it. Consider a dedicated StatuslineFormat enum.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum OutputFormat {
    /// Human-readable table format
    Table,
    /// JSON output
    Json,
    /// Claude Code statusline mode (reads context from stdin)
    #[value(name = "claude-code")]
    ClaudeCode,
}

#[derive(Parser)]
#[command(name = "wt")]
#[command(about = "Git worktree management for parallel AI agent workflows", long_about = None)]
#[command(version = version_str())]
#[command(disable_help_subcommand = true)]
#[command(styles = help_styles())]
#[command(arg_required_else_help = true)]
// Disable clap's text wrapping - we handle wrapping in the markdown renderer.
// This prevents clap from breaking markdown tables by wrapping their rows.
#[command(term_width = 0)]
#[command(after_long_help = "\
Getting started

  wt switch --create feature    # Create worktree and branch
  wt switch feature             # Switch to worktree
  wt list                       # Show all worktrees
  wt remove                     # Remove worktree; delete branch if merged

Run `wt config shell install` to set up directory switching.
Run `wt config create` to customize worktree locations.

Docs: https://worktrunk.dev
GitHub: https://github.com/max-sixty/worktrunk")]
pub(crate) struct Cli {
    /// Working directory for this command
    #[arg(
        short = 'C',
        global = true,
        value_name = "path",
        display_order = 100,
        help_heading = "Global Options"
    )]
    pub directory: Option<std::path::PathBuf>,

    /// User config file path
    #[arg(
        long,
        global = true,
        value_name = "path",
        display_order = 101,
        help_heading = "Global Options"
    )]
    pub config: Option<std::path::PathBuf>,

    /// Verbose output (-v: info logs + hook/alias template variable & output; -vv: debug logs + diagnostic report + trace.log/output.log under .git/wt/logs/)
    #[arg(
        long,
        short = 'v',
        global = true,
        action = clap::ArgAction::Count,
        display_order = 102,
        help_heading = "Global Options"
    )]
    pub verbose: u8,

    /// Skip approval prompts
    #[arg(
        long,
        short = 'y',
        global = true,
        display_order = 103,
        help_heading = "Global Options"
    )]
    pub yes: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Args)]
pub(crate) struct SwitchArgs {
    /// Branch name or shortcut
    ///
    /// Opens interactive picker if omitted.
    /// Shortcuts: '^' (default branch), '-' (previous), '@' (current), 'pr:{N}' (GitHub PR), 'mr:{N}' (GitLab MR)
    #[arg(add = crate::completion::worktree_branch_completer())]
    pub(crate) branch: Option<String>,

    /// Include branches without worktrees
    #[arg(long, help_heading = "Picker Options", conflicts_with_all = ["create", "base", "execute", "execute_args", "clobber"])]
    pub(crate) branches: bool,

    /// Include remote branches
    #[arg(long, help_heading = "Picker Options", conflicts_with_all = ["create", "base", "execute", "execute_args", "clobber"])]
    pub(crate) remotes: bool,

    /// Create a new branch
    #[arg(short = 'c', long, requires = "branch")]
    pub(crate) create: bool,

    /// Base branch
    ///
    /// Defaults to default branch. Supports the same shortcuts as the branch
    /// argument: `^`, `@`, `-`, `pr:{N}`, `mr:{N}`.
    #[arg(short = 'b', long, requires = "branch", add = crate::completion::branch_value_completer())]
    pub(crate) base: Option<String>,

    /// Command to run after switch
    ///
    /// Replaces the wt process with the command after switching, giving
    /// it full terminal control. Useful for launching editors, AI agents,
    /// or other interactive tools.
    ///
    /// Supports [hook template variables](@/hook.md#template-variables)
    /// (`{{ branch }}`, `{{ worktree_path }}`, etc.) and filters.
    /// `{{ base }}` and `{{ base_worktree_path }}` require `--create`.
    ///
    /// Especially useful with shell aliases:
    ///
    /// ```sh
    /// alias wsc='wt switch --create -x claude'
    /// wsc feature-branch -- 'Fix GH #322'
    /// ```
    ///
    /// Then `wsc feature-branch` creates the worktree and launches Claude
    /// Code. Arguments after `--` are passed to the command, so
    /// `wsc feature -- 'Fix GH #322'` runs `claude 'Fix GH #322'`,
    /// starting Claude with a prompt.
    ///
    /// Template example: `-x 'code {{ worktree_path }}'` opens VS Code
    /// at the worktree, `-x 'tmux new -s {{ branch | sanitize }}'` starts
    /// a tmux session named after the branch.
    #[arg(short = 'x', long, requires = "branch")]
    pub(crate) execute: Option<String>,

    /// Additional arguments for --execute command (after --)
    ///
    /// Arguments after `--` are appended to the execute command.
    /// Each argument is expanded for templates, then POSIX shell-escaped.
    #[arg(last = true, requires = "execute")]
    pub(crate) execute_args: Vec<String>,

    /// Remove stale paths at target
    #[arg(long, requires = "branch")]
    pub(crate) clobber: bool,

    /// Skip directory change after switching
    ///
    /// Hooks still run normally. Useful when hooks handle navigation
    /// (e.g., tmux workflows) or for CI/automation. Use --cd to override.
    ///
    /// In picker mode (no branch argument), prints the selected branch
    /// name and exits without switching. Useful for scripting.
    #[arg(long, overrides_with = "cd")]
    pub(crate) no_cd: bool,

    /// Change directory after switching
    #[arg(long, overrides_with = "no_cd", hide = true)]
    pub(crate) cd: bool,

    /// Skip hooks
    #[arg(long = "no-hooks", action = clap::ArgAction::SetFalse, default_value_t = true, help_heading = "Automation")]
    pub(crate) verify: bool,

    /// Skip hooks (deprecated alias for --no-hooks)
    #[arg(long = "no-verify", hide = true)]
    pub(crate) no_verify_deprecated: bool,

    /// Output format
    ///
    /// JSON prints structured result to stdout. Designed for tool
    /// integration (e.g., Claude Code WorktreeCreate hooks).
    #[arg(long, default_value = "text", requires = "branch", conflicts_with_all = ["branches", "remotes"], help_heading = "Automation")]
    pub(crate) format: SwitchFormat,
}

#[derive(Args)]
pub(crate) struct ListArgs {
    #[command(subcommand)]
    pub(crate) subcommand: Option<ListSubcommand>,

    /// Output format (table, json)
    #[arg(long, value_enum, default_value = "table", hide_possible_values = true)]
    pub(crate) format: OutputFormat,

    /// Include branches without worktrees
    #[arg(long)]
    pub(crate) branches: bool,

    /// Include remote branches
    #[arg(long)]
    pub(crate) remotes: bool,

    /// Show CI, diff analysis, and LLM summaries
    #[arg(long)]
    pub(crate) full: bool,

    /// Show fast info immediately, update with slow info
    ///
    /// Displays local data (branches, paths, status) first, then updates
    /// with remote data (CI, upstream) as it arrives. Use --no-progressive
    /// to force buffered rendering. Auto-enabled for TTY.
    #[arg(long, overrides_with = "no_progressive")]
    pub(crate) progressive: bool,

    /// Force buffered rendering
    #[arg(long = "no-progressive", overrides_with = "progressive", hide = true)]
    pub(crate) no_progressive: bool,
}

#[derive(Args)]
pub(crate) struct RemoveArgs {
    /// Branch name [default: current]
    #[arg(add = crate::completion::local_branches_completer())]
    pub(crate) branches: Vec<String>,

    /// Keep branch after removal
    #[arg(long = "no-delete-branch", action = clap::ArgAction::SetFalse, default_value_t = true)]
    pub(crate) delete_branch: bool,

    /// Delete unmerged branches
    #[arg(short = 'D', long = "force-delete")]
    pub(crate) force_delete: bool,

    /// Run removal in foreground (block until complete)
    #[arg(long)]
    pub(crate) foreground: bool,

    /// Skip hooks
    #[arg(long = "no-hooks", action = clap::ArgAction::SetFalse, default_value_t = true, help_heading = "Automation")]
    pub(crate) verify: bool,

    /// Skip hooks (deprecated alias for --no-hooks)
    #[arg(long = "no-verify", hide = true)]
    pub(crate) no_verify_deprecated: bool,

    /// Force worktree removal
    ///
    /// Remove worktrees even if they contain untracked files (like build
    /// artifacts). Without this flag, removal fails if untracked files exist.
    #[arg(short, long)]
    pub(crate) force: bool,

    /// Output format
    ///
    /// JSON prints structured result to stdout after removal completes.
    #[arg(long, default_value = "text", help_heading = "Automation")]
    pub(crate) format: SwitchFormat,
}

#[derive(Args)]
pub(crate) struct MergeArgs {
    /// Target branch
    ///
    /// Defaults to default branch.
    #[arg(add = crate::completion::branch_value_completer())]
    pub(crate) target: Option<String>,

    /// Force commit squashing
    #[arg(long, overrides_with = "no_squash", hide = true)]
    pub(crate) squash: bool,

    /// Skip commit squashing
    #[arg(long = "no-squash", overrides_with = "squash")]
    pub(crate) no_squash: bool,

    /// Force commit and squash
    #[arg(long, overrides_with = "no_commit", hide = true)]
    pub(crate) commit: bool,

    /// Skip commit and squash
    #[arg(long = "no-commit", overrides_with = "commit")]
    pub(crate) no_commit: bool,

    /// Force rebasing onto target
    #[arg(long, overrides_with = "no_rebase", hide = true)]
    pub(crate) rebase: bool,

    /// Skip rebase (fail if not already rebased)
    #[arg(long = "no-rebase", overrides_with = "rebase")]
    pub(crate) no_rebase: bool,

    /// Force worktree removal after merge
    #[arg(long, overrides_with = "no_remove", hide = true)]
    pub(crate) remove: bool,

    /// Keep worktree after merge
    #[arg(long = "no-remove", overrides_with = "remove")]
    pub(crate) no_remove: bool,

    /// Create a merge commit (no fast-forward)
    #[arg(long = "no-ff", overrides_with = "ff")]
    pub(crate) no_ff: bool,

    /// Allow fast-forward (default)
    #[arg(long, overrides_with = "no_ff", hide = true)]
    pub(crate) ff: bool,

    /// Force running hooks
    #[arg(long, overrides_with_all = ["no_hooks", "no_verify"], hide = true)]
    pub(crate) verify: bool,

    /// Skip hooks
    #[arg(
        long = "no-hooks",
        overrides_with_all = ["verify", "no_verify"],
        help_heading = "Automation"
    )]
    pub(crate) no_hooks: bool,

    /// Skip hooks (deprecated alias for --no-hooks)
    #[arg(long = "no-verify", overrides_with_all = ["verify", "no_hooks"], hide = true)]
    pub(crate) no_verify: bool,

    /// What to stage before committing [default: all]
    #[arg(long)]
    pub(crate) stage: Option<crate::commands::commit::StageMode>,

    /// Output format
    ///
    /// JSON prints structured result to stdout after merge completes.
    #[arg(long, default_value = "text", help_heading = "Automation")]
    pub(crate) format: SwitchFormat,
}

// Ordering: by "core-ness". Primitive worktree operations first (switch, list,
// remove), then composites built on top (merge), then subcommand namespaces
// (step, hook, config). `remove` is a primitive and more core than `merge`,
// which wraps it. Hidden commands last.
#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Switch to a worktree; create if needed
    #[command(
        after_long_help = r#"Worktrees are addressed by branch name; paths are computed from a configurable template. Unlike `git switch`, this navigates between worktrees rather than changing branches in place.

<!-- demo: wt-switch.gif 1600x900 -->
## Examples

```console
$ wt switch feature-auth           # Switch to worktree
$ wt switch -                      # Previous worktree (like cd -)
$ wt switch --create new-feature   # Create new branch and worktree
$ wt switch --create hotfix --base production
$ wt switch pr:123                 # Switch to PR #123's branch
```

## Creating a branch

The `--create` flag creates a new branch from `--base` — the default branch unless specified. Without `--create`, the branch must already exist. Switching to a remote branch (e.g., `wt switch feature` when only `origin/feature` exists) creates a local tracking branch.

## Creating worktrees

If the branch already has a worktree, `wt switch` changes directories to it. Otherwise, it creates one:

1. Runs [pre-switch hooks](@/hook.md#hook-types), blocking until complete
2. Creates worktree at configured path
3. Switches to new directory
4. Runs [pre-start hooks](@/hook.md#hook-types), blocking until complete
5. Spawns [post-start](@/hook.md#hook-types) and [post-switch hooks](@/hook.md#hook-types) in the background

```console
$ wt switch feature                        # Existing branch → creates worktree
$ wt switch --create feature               # New branch and worktree
$ wt switch --create fix --base release    # New branch from release
$ wt switch --create temp --no-hooks       # Skip hooks
```

## Shortcuts

| Shortcut | Meaning |
|----------|---------|
| `^` | Default branch (`main`/`master`) |
| `@` | Current branch/worktree |
| `-` | Previous worktree (like `cd -`) |
| `pr:{N}` | GitHub PR #N's branch |
| `mr:{N}` | GitLab MR !N's branch |

```console
$ wt switch -                           # Back to previous
$ wt switch ^                           # Default branch worktree
$ wt switch --create fix --base=@       # Branch from current HEAD
$ wt switch --create fix --base=pr:123  # Branch from PR #123's head
$ wt switch pr:123                      # PR #123's branch
$ wt switch mr:101                      # MR !101's branch
```

Shortcuts also apply to `--base`. For a fork PR/MR, the head commit is fetched and used as the base SHA without creating a tracking branch.

## Interactive picker

When called without arguments, `wt switch` opens an interactive picker to browse and select worktrees with live preview.

<!-- demo: wt-switch-picker.gif 1600x800 -->
**Keybindings:**

| Key | Action |
|-----|--------|
| `↑`/`↓` | Navigate worktree list |
| (type) | Filter worktrees |
| `Enter` | Switch to selected worktree |
| `Alt-c` | Create new worktree named as entered text |
| `Esc` | Cancel |
| `1`–`5` | Switch preview tab |
| `Alt-p` | Toggle preview panel |
| `Ctrl-u`/`Ctrl-d` | Scroll preview up/down |
<!-- Alt-r (remove worktree) works but is omitted: cursor resets after skim reload (#1695). Add once fixed. See #1881. -->

**Preview tabs** — toggle with number keys:

1. **HEAD±** — Diff of uncommitted changes
2. **log** — Recent commits; commits already on the default branch have dimmed hashes
3. **main…±** — Diff of changes since the merge-base with the default branch
4. **remote⇅** — Ahead/behind diff vs upstream tracking branch
5. **summary** — LLM-generated branch summary; requires `[list] summary = true` and `[commit.generation]`

**Pager configuration:** The preview panel pipes diff output through git's pager. Override in user config:

```toml
[switch.picker]
pager = "delta --paging=never --width=$COLUMNS"
```

Available on Unix only (macOS, Linux). On Windows, use `wt list` or `wt switch <branch>` directly.

## Pull requests and merge requests

The `pr:<number>` and `mr:<number>` shortcuts resolve a GitHub PR or GitLab MR to its branch. For same-repo PRs/MRs, worktrunk switches to the branch directly. For fork PRs/MRs, it fetches the ref (`refs/pull/N/head` or `refs/merge-requests/N/head`) and configures `pushRemote` to the fork URL.

```console
$ wt switch pr:101                 # GitHub PR #101
$ wt switch mr:101                 # GitLab MR !101
```

Requires `gh` (GitHub) or `glab` (GitLab) CLI to be installed and authenticated. The `--create` flag cannot be used with `pr:`/`mr:` syntax since the branch already exists.

**Forks:** The local branch uses the PR/MR's branch name directly (e.g., `feature-fix`), so `git push` works normally. If a local branch with that name already exists tracking something else, rename it first.

## When wt switch fails

- **Branch doesn't exist** — Use `--create`, or check `wt list --branches`
- **Path occupied** — Another worktree is at the target path; switch to it or remove it
- **Stale directory** — Use `--clobber` to remove a non-worktree directory at the target path

To change which branch a worktree is on, use `git switch` inside that worktree.

## See also

- [`wt list`](@/list.md) — View all worktrees
- [`wt remove`](@/remove.md) — Delete worktrees when done
- [`wt merge`](@/merge.md) — Integrate changes back to the default branch
"#
    )]
    Switch(SwitchArgs),

    /// List worktrees and their status
    #[command(
        after_long_help = r#"Shows uncommitted changes, divergence from the default branch and remote, and optional CI status and LLM summaries.

<!-- demo: wt-list.gif 1600x900 -->
The table renders progressively: branch names, paths, and commit hashes appear immediately, then status, divergence, and other columns fill in as background git operations complete.

## Full mode

`--full` adds columns that require network access or LLM calls: [CI status](#ci-status) (GitHub/GitLab pipeline pass/fail), line diffs since the merge-base, and [LLM-generated summaries](#llm-summaries) of each branch's changes.

## Examples

List all worktrees:

<!-- wt list -->
```console
$ wt list
  Branch       Status        HEAD±    main↕  Remote⇅  Commit    Age   Message
@ feature-api  +   ↕⇡     +54   -5   ↑4  ↓1   ⇡3      6814f02a  30m   Add API tests
^ main             ^⇅                         ⇡1  ⇣1  41ee0834  4d    Merge fix-auth: hardened to…
+ fix-auth         ↕|                ↑2  ↓1     |     b772e68b  5h    Add secure token storage
+ fix-typos        _|                           |     41ee0834  4d    Merge fix-auth: hardened to…

○ Showing 4 worktrees, 1 with changes, 2 ahead, 1 column hidden
```

Include CI status, line diffs, and LLM summaries:

<!-- wt list --full -->
```console
$ wt list --full
  Branch       Status        HEAD±    main↕     main…±  Summary                                              Remote⇅  CI  Commit
@ feature-api  +   ↕⇡     +54   -5   ↑4  ↓1  +234  -24  Refactor API to REST architecture with middleware     ⇡3      ●   6814f02a
^ main             ^⇅                                                                                         ⇡1  ⇣1  ●   41ee0834
+ fix-auth         ↕|                ↑2  ↓1   +25  -11  Harden auth with constant-time token validation         |     ●   b772e68b
+ fix-typos        _|                                                                                           |     ●   41ee0834

○ Showing 4 worktrees, 1 with changes, 2 ahead, 3 columns hidden
```

Include branches that don't have worktrees:

<!-- wt list --branches --full -->
```console
$ wt list --branches --full
  Branch       Status        HEAD±    main↕     main…±  Summary                                              Remote⇅  CI  Commit
@ feature-api  +   ↕⇡     +54   -5   ↑4  ↓1  +234  -24  Refactor API to REST architecture with middleware     ⇡3      ●   6814f02a
^ main             ^⇅                                                                                         ⇡1  ⇣1  ●   41ee0834
+ fix-auth         ↕|                ↑2  ↓1   +25  -11  Harden auth with constant-time token validation         |     ●   b772e68b
+ fix-typos        _|                                                                                           |     ●   41ee0834
  exp             /↕                 ↑2  ↓1  +137       Explore GraphQL schema and resolvers                              96379229
  wip             /↕                 ↑1  ↓1   +33       Start API documentation                                           b40716dc

○ Showing 4 worktrees, 2 branches, 1 with changes, 4 ahead, 3 columns hidden
```

Output as JSON for scripting:

```console
$ wt list --format=json
```

## Columns

| Column | Shows |
|--------|-------|
| Branch | Branch name |
| Status | Compact symbols (see below) |
| HEAD± | Uncommitted changes: +added -deleted lines |
| main↕ | Commits ahead/behind default branch |
| main…± | Line diffs since the merge-base with the default branch; `--full` only |
| Summary | LLM-generated branch summary; requires `--full`, `summary = true`, and [`commit.generation`](@/config.md#commit) [experimental] |
| Remote⇅ | Commits ahead/behind tracking branch |
| CI | Pipeline status; `--full` only |
| Path | Worktree directory |
| URL | Dev server URL from project config; dimmed if port is not listening |
| Commit | Short hash (8 chars) |
| Age | Time since last commit |
| Message | Last commit message (truncated) |

Note: `main↕` and `main…±` refer to the default branch — the header label stays `main` for compactness. `main…±` uses a merge-base (three-dot) diff.

### CI status

The CI column shows GitHub/GitLab pipeline status:

| Indicator | Meaning |
|-----------|---------|
| `●` green | All checks passed |
| `●` blue | Checks running |
| `●` red | Checks failed |
| `●` yellow | Merge conflicts with base |
| `●` gray | No checks configured |
| `⚠` yellow | Fetch error (rate limit, network) |
| (blank) | No upstream or no PR/MR |

CI indicators are clickable links to the PR or pipeline page. Any CI dot appears dimmed when unpushed local changes make the status stale. PRs/MRs are checked first, then branch workflows/pipelines for branches with an upstream. Local-only branches show blank; remote-only branches — visible with `--remotes` — get CI status detection. Results are cached for 30-60 seconds; use `wt config state` to view or clear.

### LLM summaries [experimental]

Reuses the [`commit.generation`](@/config.md#commit) command — the same LLM that generates commit messages. Enable with `summary = true` in `[list]` config; requires `--full`. Results are cached until the branch's diff changes.

## Status symbols

The Status column has multiple subcolumns. Within each, only the first matching symbol is shown (listed in priority order):

| Subcolumn | Symbol | Meaning |
|-----------|--------|---------|
| Working tree (1) | `+` | Staged files |
| Working tree (2) | `!` | Modified files (unstaged) |
| Working tree (3) | `?` | Untracked files |
| Worktree | `✘` | Merge conflicts |
| | `⤴` | Rebase in progress |
| | `⤵` | Merge in progress |
| | `/` | Branch without worktree |
| | `⚑` | Branch-worktree mismatch (branch name doesn't match worktree path) |
| | `⊟` | Prunable (directory missing) |
| | `⊞` | Locked worktree |
| Default branch | `^` | Is the default branch |
| | `∅` | Orphan branch (no common ancestor with the default branch) |
| | `✗` | Would conflict if merged to the default branch; with `--full`, includes uncommitted changes |
| | `_` | Same commit as the default branch, clean |
| | `–` | Same commit as the default branch, uncommitted changes |
| | `⊂` | Content [integrated](@/remove.md#branch-cleanup) into the default branch or target |
| | `↕` | Diverged from the default branch |
| | `↑` | Ahead of the default branch |
| | `↓` | Behind the default branch |
| Remote | `\|` | In sync with remote |
| | `⇅` | Diverged from remote |
| | `⇡` | Ahead of remote |
| | `⇣` | Behind remote |

Rows are dimmed when [safe to delete](@/remove.md#branch-cleanup) (`_` same commit with clean working tree or `⊂` content integrated).

### Placeholder symbols

These appear across all columns while the table is loading:

| Symbol | Meaning |
|--------|---------|
| `·` | Data is loading, or collection timed out / branch too stale |

---

## JSON output

Query structured data with `--format=json`:

```console
# Current worktree path (for scripts)
$ wt list --format=json | jq -r '.[] | select(.is_current) | .path'

# Branches with uncommitted changes
$ wt list --format=json | jq '.[] | select(.working_tree.modified)'

# Worktrees with merge conflicts
$ wt list --format=json | jq '.[] | select(.operation_state == "conflicts")'

# Branches ahead of main (needs merging)
$ wt list --format=json | jq '.[] | select(.main.ahead > 0) | .branch'

# Integrated branches (safe to remove)
$ wt list --format=json | jq '.[] | select(.main_state == "integrated" or .main_state == "empty") | .branch'

# Branches without worktrees
$ wt list --format=json --branches | jq '.[] | select(.kind == "branch") | .branch'

# Worktrees ahead of remote (needs pushing)
$ wt list --format=json | jq '.[] | select(.remote.ahead > 0) | {branch, ahead: .remote.ahead}'

# Stale CI (local changes not reflected in CI)
$ wt list --format=json --full | jq '.[] | select(.ci.stale) | .branch'
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `branch` | string/null | Branch name (null for detached HEAD) |
| `path` | string | Worktree path (absent for branches without worktrees) |
| `kind` | string | `"worktree"` or `"branch"` |
| `commit` | object | Commit info (see below) |
| `working_tree` | object | Working tree state (see below) |
| `main_state` | string | Relation to the default branch (see below) |
| `integration_reason` | string | Why branch is integrated (see below) |
| `operation_state` | string | `"conflicts"`, `"rebase"`, or `"merge"`; absent when clean |
| `main` | object | Relationship to the default branch (see below); absent when is_main |
| `remote` | object | Tracking branch info (see below); absent when no tracking |
| `worktree` | object | Worktree metadata (see below) |
| `is_main` | boolean | Is the main worktree |
| `is_current` | boolean | Is the current worktree |
| `is_previous` | boolean | Previous worktree from wt switch |
| `ci` | object | CI status (see below); absent when no CI |
| `url` | string | Dev server URL from project config; absent when not configured |
| `url_active` | boolean | Whether the URL's port is listening; absent when not configured |
| `summary` | string | LLM-generated branch summary; absent when not configured or no summary |
| `statusline` | string | Pre-formatted status with ANSI colors |
| `symbols` | string | Raw status symbols without colors (e.g., `"!?↓"`) |
| `vars` | object | Per-branch variables from [`wt config state vars`](@/config.md#wt-config-state-vars) (absent when empty) |

### Commit object

| Field | Type | Description |
|-------|------|-------------|
| `sha` | string | Full commit SHA (40 chars) |
| `short_sha` | string | Short commit SHA (7 chars) |
| `message` | string | Commit message (first line) |
| `timestamp` | number | Unix timestamp |

### working_tree object

| Field | Type | Description |
|-------|------|-------------|
| `staged` | boolean | Has staged files |
| `modified` | boolean | Has modified files (unstaged) |
| `untracked` | boolean | Has untracked files |
| `renamed` | boolean | Has renamed files |
| `deleted` | boolean | Has deleted files |
| `diff` | object | Lines changed vs HEAD: `{added, deleted}` |

### main object

| Field | Type | Description |
|-------|------|-------------|
| `ahead` | number | Commits ahead of the default branch |
| `behind` | number | Commits behind the default branch |
| `diff` | object | Lines changed vs the default branch: `{added, deleted}` |

### remote object

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Remote name (e.g., `"origin"`) |
| `branch` | string | Remote branch name |
| `ahead` | number | Commits ahead of remote |
| `behind` | number | Commits behind remote |

### worktree object

| Field | Type | Description |
|-------|------|-------------|
| `state` | string | `"no_worktree"`, `"branch_worktree_mismatch"`, `"prunable"`, `"locked"` (absent when normal) |
| `reason` | string | Reason for locked/prunable state |
| `detached` | boolean | HEAD is detached |

### ci object

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | CI status (see below) |
| `source` | string | `"pr"` (PR/MR) or `"branch"` (branch workflow) |
| `stale` | boolean | Local HEAD differs from remote (unpushed changes) |
| `url` | string | URL to the PR/MR page |

### main_state values

These values describe the relation to the default branch.

`"is_main"` `"orphan"` `"would_conflict"` `"empty"` `"same_commit"` `"integrated"` `"diverged"` `"ahead"` `"behind"`

### integration_reason values

When `main_state == "integrated"`: `"ancestor"` `"trees_match"` `"no_added_changes"` `"merge_adds_nothing"` `"patch-id-match"`

### ci.status values

`"passed"` `"running"` `"failed"` `"conflicts"` `"no-ci"` `"error"`

Missing a field that would be generally useful? Open an issue at https://github.com/max-sixty/worktrunk.

## See also

- [`wt switch`](@/switch.md) — Switch worktrees or open interactive picker
"#
    )]
    // TODO: `args_conflicts_with_subcommands` causes confusing errors for unknown
    // subcommands ("cannot be used with --branches") instead of "unknown subcommand".
    // Could fix with external_subcommand + post-parse validation, but not worth the
    // code. The `statusline` subcommand may move elsewhere anyway.
    #[command(args_conflicts_with_subcommands = true)]
    List(ListArgs),

    /// Remove worktree; delete branch if merged
    ///
    /// Defaults to the current worktree.
    #[command(after_long_help = r#"## Examples

Remove current worktree:

<!-- wt remove (docs-example) -->
```console
$ wt remove
◎ Running pre-remove project:cleanup
  flyctl scale count 0
Scaling app to 0 machines
◎ Removing api worktree & branch in background (same commit as main, _)
○ Switched to worktree for main @ ~/repo
```

Remove specific worktrees / branches:

```console
$ wt remove feature-branch
$ wt remove old-feature another-branch
```

Keep the branch:

```console
$ wt remove --no-delete-branch feature-branch
```

Force-delete an unmerged branch:

```console
$ wt remove -D experimental
```

## Branch cleanup

By default, branches are deleted when they would add no changes to the default branch if merged. This works with both unchanged git histories, and squash-merge or rebase workflows where commit history differs but file changes match.

Worktrunk checks six conditions (in order of cost):

1. **Same commit** — Branch HEAD equals the default branch. Shows `_` in `wt list`.
2. **Ancestor** — Branch is in target's history (fast-forward or rebase case). Shows `⊂`.
3. **No added changes** — Three-dot diff (`target...branch`) is empty. Shows `⊂`.
4. **Trees match** — Branch tree SHA equals target tree SHA. Shows `⊂`.
5. **Merge adds nothing** — Simulated merge produces the same tree as target. Handles squash-merged branches where target has advanced with changes to different files. Shows `⊂`.
6. **Patch-id match** — Branch's entire diff matches a single squash-merge commit on target. Fallback for when the simulated merge conflicts because target later modified the same files the branch touched. Shows `⊂`.

The 'same commit' check uses the local default branch; for other checks, 'target' means the default branch, or its upstream (e.g., `origin/main`) when strictly ahead.

Branches matching these conditions and with empty working trees are dimmed in `wt list` as safe to delete.

## Force flags

Worktrunk has two force flags for different situations:

| Flag | Scope | When to use |
|------|-------|-------------|
| `--force` (`-f`) | Worktree | Worktree has untracked files |
| `--force-delete` (`-D`) | Branch | Branch has unmerged commits |

```console
$ wt remove feature --force       # Remove worktree with untracked files
$ wt remove feature -D            # Delete unmerged branch
$ wt remove feature --force -D    # Both
```

Without `--force`, removal fails if the worktree contains untracked files. Without `--force-delete`, removal keeps branches with unmerged changes. Use `--no-delete-branch` to keep the branch regardless of merge status.

## Background removal

Removal runs in the background by default — the command returns immediately. The worktree is renamed into `.git/wt/trash/` (instant same-filesystem rename), git metadata is pruned, the branch is deleted, and a detached `rm -rf` finishes cleanup. Cross-filesystem worktrees fall back to `git worktree remove`. Logs: `.git/wt/logs/{branch}/internal/remove.log`. Use `--foreground` to run in the foreground.

After each `wt remove`, entries in `.git/wt/trash/` older than 24 hours are swept by a detached `rm -rf` — eventual cleanup for directories orphaned when a previous background removal was interrupted (SIGKILL, reboot, disk full).

## Hooks

`pre-remove` hooks run before the worktree is deleted (with access to worktree files). `post-remove` hooks run after removal. See [`wt hook`](@/hook.md) for configuration.

## Detached HEAD worktrees

Detached worktrees have no branch name. Pass the worktree path instead: `wt remove /path/to/worktree`.

## See also

- [`wt merge`](@/merge.md) — Remove worktree after merging
- [`wt list`](@/list.md) — View all worktrees
"#)]
    Remove(RemoveArgs),

    /// Merge current branch into the target branch
    ///
    /// Squash & rebase, fast-forward the target branch, remove the worktree.
    #[command(
        after_long_help = r#"Unlike `git merge`, this merges the current branch into the target branch — not the target into current. Similar to clicking "Merge pull request" on GitHub, but locally. The target defaults to the default branch.

<!-- demo: wt-merge.gif 1600x900 -->
## Examples

Merge to the default branch:

<!-- wt merge (docs-example) -->
```console
$ wt merge
◎ Running pre-merge project:test
  cargo nextest run
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.02s
     Summary [   0.002s] 2 tests run: 2 passed, 0 skipped
◎ Merging 1 commit to main @ a1b2c3d (no commit/squash/rebase needed)
  * a1b2c3d feat: add hook registration
   hook.rs | 31 +++++++++++++++++++++++++++++++
   1 file changed, 31 insertions(+)
✓ Merged to main (1 commit, 1 file, +31)
◎ Removing hooks worktree & branch in background (same commit as main, _)
○ Switched to worktree for main @ ~/repo
```

Merge to a different branch:

```console
$ wt merge develop
```

Keep the worktree after merging:

```console
$ wt merge --no-remove
```

Preserve commit history (no squash):

```console
$ wt merge --no-squash
```

Create a merge commit — semi-linear history:

```console
$ wt merge --no-ff
```

Skip committing/squashing (rebase still runs unless --no-rebase):

```console
$ wt merge --no-commit
```

## Pipeline

`wt merge` runs these steps:

1. **Commit** — Pre-commit hooks run, then uncommitted changes are committed. Post-commit hooks run in background. Skipped when squashing (the default) — changes are staged during the squash step instead. With `--no-squash`, this is the only commit step.
2. **Squash** — Combines all commits since target into one (like GitHub's "Squash and merge"). Use `--stage` to control what gets staged: `all` (default), `tracked`, or `none`. A backup ref is saved to `refs/wt-backup/<branch>`. With `--no-squash`, individual commits are preserved.
3. **Rebase** — Rebases onto target if behind. Skipped if already up-to-date. Conflicts abort immediately.
4. **Pre-merge hooks** — Hooks run after rebase, before merge. Failures abort. See [`wt hook`](@/hook.md).
5. **Merge** — Fast-forward merge to the target branch. With `--no-ff`, a merge commit is created instead — semi-linear history with rebased commits plus a merge commit. Non-fast-forward merges are rejected.
6. **Pre-remove hooks** — Hooks run before removing worktree. Failures abort.
7. **Cleanup** — Removes the worktree and branch. Use `--no-remove` to keep the worktree. When already on the target branch or in the primary worktree, the worktree is preserved.
8. **Post-remove + post-merge hooks** — Run in background after cleanup.

Use `--no-commit` to skip committing uncommitted changes and squashing; rebase still runs by default and can rewrite commits unless `--no-rebase` is passed. Useful after preparing commits manually with `wt step commit`. Requires a clean working tree.

## Local CI

For personal projects, pre-merge hooks open up the possibility of a workflow with much faster iteration — an order of magnitude more small changes instead of fewer large ones.

Historically, ensuring tests ran before merging was difficult to enforce locally. Remote CI was valuable for the process as much as the checks: it guaranteed validation happened. `wt merge` brings that guarantee local.

The full workflow: start an agent (one of many) on a task, work elsewhere, return when it's ready. Review the diff, run `wt merge`, move on. Pre-merge hooks validate before merging — if they pass, the branch goes to the default branch and the worktree cleans up.

```toml
[[pre-merge]]
test = "cargo test"
lint = "cargo clippy"
```

## See also

- [`wt step`](@/step.md) — Run individual operations (commit, squash, rebase, push)
- [`wt remove`](@/remove.md) — Remove worktrees without merging
- [`wt switch`](@/switch.md) — Navigate to other worktrees
"#
    )]
    Merge(MergeArgs),
    /// Deprecated: use `wt switch` instead
    ///
    /// Interactive worktree picker (now integrated into `wt switch`).
    #[command(hide = true)]
    Select {
        /// Include branches without worktrees
        #[arg(long)]
        branches: bool,

        /// Include remote branches
        #[arg(long)]
        remotes: bool,
    },

    /// Run individual operations
    ///
    /// The building blocks of `wt merge` — commit, squash, rebase, push — plus standalone utilities.
    #[command(
        name = "step",
        arg_required_else_help = true,
        after_long_help = r#"## Examples

Commit with LLM-generated message:

<!-- wt step commit (docs-example) -->
```console
$ wt step commit
◎ Generating commit message and committing changes... (2 files, +26)
  feat(validation): add input validation utilities
✓ Committed changes @ a1b2c3d
```

Manual merge workflow with review between steps:

```console
$ wt step commit
$ wt step squash
$ wt step rebase
$ wt step push
```

## Operations

- [`commit`](#wt-step-commit) — Stage and commit with [LLM-generated message](@/llm-commits.md)
- [`squash`](#wt-step-squash) — Squash all branch commits into one with [LLM-generated message](@/llm-commits.md)
- `rebase` — Rebase onto target branch
- `push` — Fast-forward target to current branch
- [`diff`](#wt-step-diff) — Show all changes since branching (committed, staged, unstaged, untracked)
- [`copy-ignored`](#wt-step-copy-ignored) — Copy gitignored files between worktrees
- [`eval`](#wt-step-eval) — [experimental] Evaluate a template expression
- [`for-each`](#wt-step-for-each) — [experimental] Run a command in every worktree
- [`promote`](#wt-step-promote) — [experimental] Swap a branch into the main worktree
- [`prune`](#wt-step-prune) — Remove worktrees and branches merged into the default branch
- [`relocate`](#wt-step-relocate) — [experimental] Move worktrees to expected paths
- [`<alias>`](@/extending.md#aliases) — Run a configured command alias

## See also

- [`wt merge`](@/merge.md) — Runs commit → squash → rebase → hooks → push → cleanup automatically
- [`wt hook`](@/hook.md) — Run configured hooks
- [Aliases](@/extending.md#aliases) — Custom command templates run as `wt <name>`
<!-- subdoc: commit -->
<!-- subdoc: squash -->
<!-- subdoc: diff -->
<!-- subdoc: copy-ignored -->
<!-- subdoc: eval -->
<!-- subdoc: for-each -->
<!-- subdoc: promote -->
<!-- subdoc: prune -->
<!-- subdoc: relocate -->"#
    )]
    Step {
        #[command(subcommand)]
        action: StepCommand,
    },

    /// Run configured hooks
    #[command(
        name = "hook",
        after_long_help = r#"Hooks are shell commands that run at key points in the worktree lifecycle — automatically during `wt switch`, `wt merge`, & `wt remove`, or on demand via `wt hook <type>`. Both user and project hooks are supported.

# Hook Types

| Event | `pre-` — blocking | `post-` — background |
|-------|-------------------|---------------------|
| **switch** | `pre-switch` | `post-switch` |
| **start** | `pre-start` | `post-start` |
| **commit** | `pre-commit` | `post-commit` |
| **merge** | `pre-merge` | `post-merge` |
| **remove** | `pre-remove` | `post-remove` |

`pre-*` hooks block — failure aborts the operation. `post-*` hooks run in the background with output logged (use [`wt config state logs`](@/config.md#wt-config-state-logs) to find and manage log files). Use `-v` to see expanded command details for background hooks.

The most common starting point is `post-start` — it runs background tasks (dev servers, file copying, builds) without blocking worktree creation. Prefer `post-start` over `pre-start` unless a later step needs the work completed first.

| Hook | Purpose |
|------|---------|
| `pre-switch` | Runs before branch resolution or worktree creation. `{{ branch }}` is the destination as typed (before resolution) |
| `post-switch` | Triggers on all switch results: creating, switching to existing, or staying on current |
| `pre-start` | Runs once when a new worktree is created, blocking `post-start`/`--execute` until complete: dependency install, env file generation |
| `post-start` | Runs once when a new worktree is created, in the background: dev servers, long builds, file watchers, copying caches |
| `pre-commit` | Formatters, linters, type checking — runs during `wt merge` before the squash commit |
| `post-commit` | CI triggers, notifications, background linting |
| `pre-merge` | Tests, security scans, build verification — runs after rebase, before merge to target |
| `post-merge` | Deployment, notifications, installing updated binaries. Runs in the target branch worktree if it exists, otherwise the primary worktree |
| `pre-remove` | Cleanup before worktree deletion: saving test artifacts, backing up state. Runs in the worktree being removed |
| `post-remove` | Stopping dev servers, removing containers, notifying external systems. Template variables reference the removed worktree |

During `wt merge`, hooks run in this order: pre-commit → post-commit → pre-merge → pre-remove → post-remove + post-merge. As usual, post-* hooks run in the background. See [`wt merge`](@/merge.md#pipeline) for the complete pipeline.

# Security

Project commands require approval on first run:

```
▲ repo needs approval to execute 3 commands:

○ pre-start install:
   npm ci
○ pre-start build:
   cargo build --release
○ pre-start env:
   echo 'PORT={{ branch | hash_port }}' > .env.local

❯ Allow and remember? [y/N]
```

- Approvals are saved to `~/.config/worktrunk/approvals.toml`
- If a command changes, new approval is required
- Use `--yes` to bypass prompts — useful for CI and automation
- Use `--no-hooks` to skip hooks

Manage approvals with `wt config approvals add` and `wt config approvals clear`.

# Configuration

Hooks can be defined in project config (`.config/wt.toml`) or user config (`~/.config/worktrunk/config.toml`). Both use the same format.

## Hook forms

Hooks take one of three forms, determined by their TOML shape.

A string is a single command:

```toml
pre-start = "npm install"
```

A table is multiple commands that run concurrently:

```toml
[post-start]
server = "npm run dev"
watch = "npm run watch"
```

A pipeline is a sequence of `[[hook]]` blocks run in order. Each block is one step; multiple keys within a block run concurrently. A failing step aborts the rest of the pipeline:

```toml
[[post-start]]
install = "npm ci"

[[post-start]]
build = "npm run build"
server = "npm run dev"
```

Here `install` runs first, then `build` and `server` run together.

Most hooks don't need `[[hook]]` blocks. Reach for them when there's a dependency chain — typically setup that must complete before later steps, like installing dependencies before running a build and dev server concurrently.

Table form for pre-* hooks is deprecated and its behavior will change in a future version — use `[[hook]]` blocks instead.

## Project vs user hooks

| Aspect | Project hooks | User hooks |
|--------|--------------|------------|
| Location | `.config/wt.toml` | `~/.config/worktrunk/config.toml` |
| Scope | Single repository | All repositories (or [per-project](@/config.md#user-project-specific-settings)) |
| Approval | Required | Not required |
| Execution order | After user hooks | First |

Skip all hooks with `--no-hooks`. To run a specific hook when user and project both define the same name, use `user:name` or `project:name` syntax.

## Template variables

Hooks can use template variables that expand at runtime:

| Kind | Variable | Description |
|------|----------|-------------|
| active    | `{{ branch }}`                | Branch name |
|           | `{{ worktree_path }}`         | Worktree path |
|           | `{{ worktree_name }}`         | Worktree directory name |
|           | `{{ commit }}`                | Branch HEAD SHA |
|           | `{{ short_commit }}`          | Branch HEAD SHA (7 chars) |
|           | `{{ upstream }}`              | Branch upstream (if tracking a remote) |
| operation | `{{ base }}`                  | Base branch name |
|           | `{{ base_worktree_path }}`    | Base worktree path |
|           | `{{ target }}`                | Target branch name |
|           | `{{ target_worktree_path }}`  | Target worktree path |
|           | `{{ pr_number }}`             | PR/MR number (when creating via `pr:N` / `mr:N`) |
|           | `{{ pr_url }}`                | PR/MR web URL (when creating via `pr:N` / `mr:N`) |
| repo      | `{{ repo }}`                  | Repository directory name |
|           | `{{ repo_path }}`             | Absolute path to repository root |
|           | `{{ owner }}`                 | Primary remote owner path (may include subgroups) |
|           | `{{ primary_worktree_path }}` | Primary worktree path |
|           | `{{ default_branch }}`        | Default branch name |
|           | `{{ remote }}`                | Primary remote name |
|           | `{{ remote_url }}`            | Remote URL |
| exec      | `{{ cwd }}`                   | Directory where the hook command runs |
|           | `{{ hook_type }}`             | Hook type being run (e.g. `pre-start`, `pre-merge`) |
|           | `{{ hook_name }}`             | Hook command name (if named) |
|           | `{{ args }}`                  | Tokens forwarded from the CLI — see [Running Hooks Manually](#running-hooks-manually) |
| user      | `{{ vars.<key> }}`            | Per-branch variables from [`wt config state vars`](@/config.md#wt-config-state-vars) |

Bare variables (`branch`, `worktree_path`, `commit`) refer to the branch the operation acts on: the destination for switch/create, the source for merge/remove. `base` and `target` give the other side:

| Operation | Bare vars | `base` | `target` |
|-----------|-----------|--------|----------|
| switch/create | destination | where you came from | = bare vars |
| commit (during merge/squash) | worktree being squashed | = bare vars | integration target |
| merge | feature being merged | = bare vars | merge target |
| remove | branch being removed | = bare vars | where you end up |

Pre and post hooks share the same perspective — `{{ branch | hash_port }}` produces the same port in `post-start` and `post-remove`. `cwd` is the worktree root where the hook command runs. It differs from `worktree_path` in three cases: pre-switch, where the hook runs in the source but `worktree_path` is the destination; post-remove, where the active worktree is gone so the hook runs in primary; and post-merge with removal, same — the active worktree is gone, so the hook runs in target.

Some variables are conditional: `upstream` requires remote tracking; `base` only appears in switch/create hooks; `target_worktree_path` requires the target to have a worktree; `pr_number`/`pr_url` are populated for `post-switch`, `pre-start`, and `post-start` hooks when creating via `pr:N` or `mr:N`; `vars` keys may not exist. Undefined variables error — use conditionals or defaults for optional behavior:

```toml
[pre-start]
# Rebase onto upstream if tracking a remote branch (e.g., wt switch --create feature origin/feature)
sync = "{% if upstream %}git fetch && git rebase {{ upstream }}{% endif %}"
```

Run any hook-firing command with `-v` to see the resolved variables for the actual invocation — each hook prints a `template variables:` block showing every in-scope variable and its value (`(unset)` for conditional vars that didn't populate, like `target_worktree_path` during `wt switch -`). Aliases do the same under `-v`: `wt -v <alias>` prints the alias's in-scope variables before the pipeline runs.

Variables use dot access and the `default` filter for missing keys. JSON object/array values are parsed automatically, so `{{ vars.config.port }}` works when the value is `{"port": 3000}`:

```toml
[post-start]
dev = "ENV={{ vars.env | default('development') }} npm start -- --port {{ vars.config.port | default('3000') }}"
```

## Worktrunk filters

Templates support Jinja2 filters for transforming values:

| Filter | Example | Description |
|--------|---------|-------------|
| `sanitize` | `{{ branch \| sanitize }}` | Replace `/` and `\` with `-` |
| `sanitize_db` | `{{ branch \| sanitize_db }}` | Database-safe identifier with hash suffix (`[a-z0-9_]`, max 63 chars) |
| `sanitize_hash` | `{{ branch \| sanitize_hash }}` | Filesystem-safe name with hash suffix for uniqueness |
| `hash_port` | `{{ branch \| hash_port }}` | Hash to port 10000-19999 |

The `sanitize` filter makes branch names safe for filesystem paths. The `sanitize_db` filter produces database-safe identifiers — lowercase alphanumeric and underscores, no leading digits, with a 3-character hash suffix to avoid collisions and reserved words. The `sanitize_hash` filter produces a filesystem-safe name and appends a 3-character hash suffix when sanitization changed the input, so distinct originals never collide — already-safe names pass through unchanged. The `hash_port` filter is useful for running dev servers on unique ports per worktree:

```toml
[post-start]
dev = "npm run dev -- --host {{ branch }}.localhost --port {{ branch | hash_port }}"
```

Hash any string, including concatenations:

```toml
# Unique port per repo+branch combination
dev = "npm run dev --port {{ (repo ~ '-' ~ branch) | hash_port }}"
```

Variables are shell-escaped automatically — quotes around `{{ ... }}` are unnecessary and can cause issues with special characters.

## Worktrunk functions

Templates also support functions for dynamic lookups:

| Function | Example | Description |
|----------|---------|-------------|
| `worktree_path_of_branch(branch)` | `{{ worktree_path_of_branch("main") }}` | Look up the path of a branch's worktree |

The `worktree_path_of_branch` function returns the filesystem path of a worktree given a branch name, or an empty string if no worktree exists for that branch. This is useful for referencing files in other worktrees:

```toml
[pre-start]
# Copy config from main worktree
setup = "cp {{ worktree_path_of_branch('main') }}/config.local {{ worktree_path }}"
```

## JSON context

Hooks receive all template variables as JSON on stdin, enabling complex logic that templates can't express:

```toml
[pre-start]
setup = "python3 scripts/pre-start-setup.py"
```

```python
import json, sys, subprocess
ctx = json.load(sys.stdin)
if ctx['branch'].startswith('feature/') and 'backend' in ctx['repo']:
    subprocess.run(['make', 'seed-db'])
```

## Copying untracked files

One specific command worth calling out: [`wt step copy-ignored`](@/step.md#wt-step-copy-ignored). Git worktrees share the repository but not untracked files, and this copies gitignored files between worktrees:

```toml
[post-start]
copy = "wt step copy-ignored"
```

# Running Hooks Manually

`wt hook <type>` runs hooks on demand — useful for testing during development, running in CI pipelines, or re-running after a failure.

```console
$ wt hook pre-merge              # Run all pre-merge hooks
$ wt hook pre-merge test         # Run hooks named "test" from both sources
$ wt hook pre-merge test build   # Run hooks named "test" and "build"
$ wt hook pre-merge user:        # Run all user hooks
$ wt hook pre-merge project:     # Run all project hooks
$ wt hook pre-merge user:test    # Run only user's "test" hook
$ wt hook pre-merge --yes        # Skip approval prompts (for CI)
$ wt hook pre-start --branch=feature/test    # Override a template variable
$ wt hook pre-merge -- --extra args     # Forward tokens into {{ args }}
```

The `user:` and `project:` prefixes filter by source. Use `user:` or `project:` alone to run all hooks from that source, or `user:name` / `project:name` to run a specific hook.

<!-- wt hook pre-merge (docs-example) -->
```console
$ wt hook pre-merge
◎ Running pre-merge project:test
  cargo test
    Finished test [unoptimized + debuginfo] target(s) in 0.12s
     Running unittests src/lib.rs (target/debug/deps/worktrunk-abc123)

running 18 tests
test auth::tests::test_jwt_decode ... ok
test auth::tests::test_jwt_encode ... ok
test auth::tests::test_token_refresh ... ok
test auth::tests::test_token_validation ... ok

test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
◎ Running pre-merge project:lint
  cargo clippy
    Checking worktrunk v0.1.0
    Finished dev [unoptimized + debuginfo] target(s) in 1.23s
```

```console
$ wt hook post-start
◎ Running post-start: project @ ~/acme
```

## Passing values

`--KEY=VALUE` binds `KEY` whenever `{{ KEY }}` appears in any command of the hook — the same smart-routing rule `wt <alias>` uses. Built-in variables can be overridden: `--branch=foo` sets `{{ branch }}` inside hook templates (the worktree's actual branch doesn't move). Hyphens in keys become underscores: `--my-var=x` sets `{{ my_var }}`.

Any `--KEY=VALUE` whose key isn't referenced by a hook template forwards into `{{ args }}` as a literal `--KEY=VALUE` token. Tokens after `--` also forward into `{{ args }}` verbatim. `{{ args }}` renders as a space-joined, shell-escaped string; index with `{{ args[0] }}`, loop with `{% for a in args %}…{% endfor %}`, count with `{{ args | length }}`.

The long form `--var KEY=VALUE` is deprecated but still supported. It force-binds regardless of whether any hook template references `KEY` — useful when a template only references the key conditionally (e.g. `{% if override %}…{% endif %}`).

# Recipes

- [Eliminate cold starts](@/tips-patterns.md#eliminate-cold-starts): `wt step copy-ignored` in `post-start` shares build caches and dependencies; use a `[[post-start]]` pipeline when a later hook depends on the copy
- [Dev server per worktree](@/tips-patterns.md#dev-server-per-worktree): `hash_port` in `post-start` for launch and `post-remove` for cleanup, with optional subdomain routing
- [Database per worktree](@/tips-patterns.md#database-per-worktree): a `post-start` pipeline stores container name, port, and connection string as [per-branch vars](@/config.md#wt-config-state-vars) that later hooks reference
- [Progressive validation](@/tips-patterns.md#progressive-validation): quick lint/typecheck in `pre-commit`, expensive tests and builds in `pre-merge`
- [Target-specific hooks](@/tips-patterns.md#target-specific-hooks): branch on `{{ target }}` in `post-merge` for per-environment deploys

## See also

- [`wt merge`](@/merge.md) — Runs hooks automatically during merge
- [`wt switch`](@/switch.md) — Runs pre-start/post-start hooks on `--create`
- [`wt config approvals`](@/config.md#wt-config-approvals) — Manage approvals
- [`wt config state logs`](@/config.md#wt-config-state-logs) — Access background hook logs
"#
    )]
    Hook {
        #[command(subcommand)]
        action: HookCommand,
    },

    /// Manage user & project configs
    ///
    /// Includes shell integration, hooks, and saved state.
    #[command(after_long_help = r#"## Examples

Install shell integration (required for directory switching):

```console
$ wt config shell install
```

Create user config file with documented examples:

```console
$ wt config create
```

Create project config file (`.config/wt.toml`) for hooks:

```console
$ wt config create --project
```

Show current configuration and file locations:

```console
$ wt config show
```

## Configuration files

| File | Location | Contains | Committed & shared |
|------|----------|----------|--------------------|
| **User config** | `~/.config/worktrunk/config.toml` | Worktree path template, LLM commit configs, etc | ✗ |
| **Project config** | `.config/wt.toml` | Project hooks, dev server URL | ✓ |

Organizations can also deploy a system-wide config file for shared defaults — run `wt config show` for the platform-specific location.

**User config** — personal preferences:

```toml
# ~/.config/worktrunk/config.toml
worktree-path = ".worktrees/{{ branch | sanitize }}"

[commit.generation]
command = "CLAUDECODE= MAX_THINKING_TOKENS=0 claude -p --no-session-persistence --model=haiku --tools='' --disable-slash-commands --setting-sources='' --system-prompt=''"
```

**Project config** — shared team settings:

```toml
# .config/wt.toml
[pre-start]
deps = "npm ci"

[pre-merge]
test = "npm test"
```

<!-- USER_CONFIG_START -->
# User Configuration

Create with `wt config create`. Values shown are defaults unless noted otherwise.

Location:

- macOS/Linux: `~/.config/worktrunk/config.toml` (or `$XDG_CONFIG_HOME` if set)
- Windows: `%APPDATA%\worktrunk\config.toml`

## Worktree path template

Controls where new worktrees are created.

**Available template variables:**

- `{{ repo_path }}` — absolute path to the repository root (e.g., `/Users/me/code/myproject`. Or for bare repos, the bare directory itself)
- `{{ repo }}` — repository directory name (e.g., `myproject`)
- `{{ owner }}` — primary remote owner path (may include subgroups like `group/subgroup`)
- `{{ branch }}` — raw branch name (e.g., `feature/auth`)
- `{{ branch | sanitize }}` — filesystem-safe: `/` and `\` become `-` (e.g., `feature-auth`)
- `{{ branch | sanitize_db }}` — database-safe: lowercase, underscores, hash suffix (e.g., `feature_auth_x7k`)

**Examples** for repo at `~/code/myproject`, branch `feature/auth`:

Default — sibling directory (`~/code/myproject.feature-auth`):

```toml
worktree-path = "{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}"
```

Inside the repository (`~/code/myproject/.worktrees/feature-auth`):

```toml
worktree-path = "{{ repo_path }}/.worktrees/{{ branch | sanitize }}"
```

Centralized worktrees directory (`~/worktrees/myproject/feature-auth`):

```toml
worktree-path = "~/worktrees/{{ repo }}/{{ branch | sanitize }}"
```

By remote owner path (`~/development/max-sixty/myproject/feature/auth`):

```toml
worktree-path = "~/development/{{ owner }}/{{ repo }}/{{ branch }}"
```

Bare repository (`~/code/myproject/feature-auth`):

```toml
worktree-path = "{{ repo_path }}/../{{ branch | sanitize }}"
```

`~` expands to the home directory. Relative paths resolve from `repo_path`.

## LLM commit messages

Generate commit messages automatically during merge. Requires an external CLI tool.

### Claude Code

```toml
[commit.generation]
command = "CLAUDECODE= MAX_THINKING_TOKENS=0 claude -p --no-session-persistence --model=haiku --tools='' --disable-slash-commands --setting-sources='' --system-prompt=''"
```

### Codex

```toml
[commit.generation]
command = "codex exec -m gpt-5.1-codex-mini -c model_reasoning_effort='low' -c system_prompt='' --sandbox=read-only --json - | jq -sr '[.[] | select(.item.type? == \"agent_message\")] | last.item.text'"
```

### OpenCode

```toml
[commit.generation]
command = "opencode run -m anthropic/claude-haiku-4.5 --variant fast"
```

### llm

```toml
[commit.generation]
command = "llm -m claude-haiku-4.5"
```

### aichat

```toml
[commit.generation]
command = "aichat -m claude:claude-haiku-4.5"
```

See [LLM commits docs](@/llm-commits.md) for setup and [Custom prompt templates](#custom-prompt-templates) for template customization.

## Command config

### List

Persistent flag values for `wt list`. Override on command line as needed.

```toml
[list]
summary = false    # Enable LLM branch summaries (requires [commit.generation])

full = false       # Show CI, main…± diffstat, and LLM summaries (--full)
branches = false   # Include branches without worktrees (--branches)
remotes = false    # Include remote-only branches (--remotes)

task-timeout-ms = 0   # Kill individual git commands after N ms; 0 disables
timeout-ms = 0        # Wall-clock budget for the entire collect phase; 0 disables
```

### Commit

Shared by `wt step commit`, `wt step squash`, and `wt merge`.

```toml
[commit]
stage = "all"      # What to stage before commit: "all", "tracked", or "none"
```

### Merge

Most flags are on by default. Set to false to change default behavior.

```toml
[merge]
squash = true      # Squash commits into one (--no-squash to preserve history)
commit = true      # Commit uncommitted changes first (--no-commit to skip)
rebase = true      # Rebase onto target before merge (--no-rebase to skip)
remove = true      # Remove worktree after merge (--no-remove to keep)
verify = true      # Run project hooks (--no-hooks to skip)
ff = true          # Fast-forward merge (--no-ff to create a merge commit instead)
```

### Switch

```toml
[switch]
cd = true          # Change directory after switching (--no-cd to skip)

[switch.picker]
pager = "delta --paging=never"   # Example: override git's core.pager for diff preview
```

### Step

```toml
[step.copy-ignored]
exclude = []   # Additional excludes (e.g., [".cache/", ".turbo/"])
```

Built-in excludes always apply: VCS metadata directories (`.bzr/`, `.hg/`, `.jj/`, `.pijul/`, `.sl/`, `.svn/`) and tool-state directories (`.conductor/`, `.entire/`, `.pi/`, `.worktrees/`). User config and project config exclusions are combined.

### Aliases

Command templates that run as `wt <name>`. See the [Extending Worktrunk guide](@/extending.md#aliases) for usage and flags.

```toml
[aliases]
greet = "echo Hello from {{ branch }}"
url = "echo http://localhost:{{ branch | hash_port }}"
```

Aliases defined here apply to all projects. For project-specific aliases, use the [project config](@/config.md#project-configuration) `[aliases]` section instead.

### User project-specific settings

For context:

- [Project config](@/config.md#project-configuration) settings are shared with teammates.
- User configs generally apply to all projects.
- User configs _also_ has a `[projects]` table which holds project-specific settings for the user, such as worktree layout and setting overrides. That's what this section covers.

Entries are keyed by project identifier (e.g., `github.com/user/repo`). Scalar values (like `worktree-path`) replace the global value; everything else (hooks, aliases, etc.) appends, global first.

```toml
[projects."github.com/user/repo"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
list.full = true
merge.squash = false
pre-start.env = "cp .env.example .env"
step.copy-ignored.exclude = [".repo-local-cache/"]
aliases.deploy = "make deploy BRANCH={{ branch }}"
```

### Custom prompt templates

Templates use [minijinja](https://docs.rs/minijinja/) syntax.

#### Commit template

Available variables:

- `{{ git_diff }}`, `{{ git_diff_stat }}` — diff content
- `{{ branch }}`, `{{ repo }}` — context
- `{{ recent_commits }}` — recent commit messages

Default template:

<!-- DEFAULT_TEMPLATE_START -->
```toml
[commit.generation]
template = """
<task>Write a commit message for the staged changes below.</task>

<format>
- Subject line under 50 chars
- For material changes, add a blank line then a body paragraph explaining the change
- Output only the commit message, no quotes or code blocks
</format>

<style>
- Imperative mood: "Add feature" not "Added feature"
- Match recent commit style (conventional commits if used)
- Describe the change, not the intent or benefit
</style>

<diffstat>
{{ git_diff_stat }}
</diffstat>

<diff>
{{ git_diff }}
</diff>

<context>
Branch: {{ branch }}
{% if recent_commits %}<recent_commits>
{% for commit in recent_commits %}- {{ commit }}
{% endfor %}</recent_commits>{% endif %}
</context>

"""
```
<!-- DEFAULT_TEMPLATE_END -->

#### Squash template

Available variables (in addition to commit template variables):

- `{{ commits }}` — list of commits being squashed
- `{{ target_branch }}` — merge target branch

Default template:

<!-- DEFAULT_SQUASH_TEMPLATE_START -->
```toml
[commit.generation]
squash-template = """
<task>Write a commit message for the combined effect of these commits.</task>

<format>
- Subject line under 50 chars
- For material changes, add a blank line then a body paragraph explaining the change
- Output only the commit message, no quotes or code blocks
</format>

<style>
- Imperative mood: "Add feature" not "Added feature"
- Match the style of commits being squashed (conventional commits if used)
- Describe the change, not the intent or benefit
</style>

<commits branch="{{ branch }}" target="{{ target_branch }}">
{% for commit in commits %}- {{ commit }}
{% endfor %}</commits>

<diffstat>
{{ git_diff_stat }}
</diffstat>

<diff>
{{ git_diff }}
</diff>

"""
```
<!-- DEFAULT_SQUASH_TEMPLATE_END -->

## Hooks

See [`wt hook`](@/hook.md) for hook types, execution order, template variables, and examples. User hooks apply to all projects; [project hooks](@/config.md#project-configuration) apply only to that repository.
<!-- USER_CONFIG_END -->
<!-- PROJECT_CONFIG_START -->
# Project Configuration

Project configuration lets teams share repository-specific settings — hooks, dev server URLs, and other defaults. The file lives in `.config/wt.toml` and is typically checked into version control.

To create a starter file with commented-out examples, run `wt config create --project`.

## Hooks

Project hooks apply to this repository only. See [`wt hook`](@/hook.md) for hook types, execution order, and examples.

```toml
pre-start = "npm ci"
post-start = "npm run dev"
pre-merge = "npm test"
```

## Dev server URL

URL column in `wt list` (dimmed when port not listening):

```toml
[list]
url = "http://localhost:{{ branch | hash_port }}"
```

## Forge platform override

Override platform detection for SSH aliases or self-hosted instances:

```toml
[forge]
platform = "github"  # or "gitlab"
hostname = "github.example.com"  # Example: API host (GHE / self-hosted GitLab)
```

## Copy-ignored excludes

Additional excludes for `wt step copy-ignored`:

```toml
[step.copy-ignored]
exclude = [".cache/", ".turbo/"]
```

Built-in excludes always apply: VCS metadata directories (`.bzr/`, `.hg/`, `.jj/`, `.pijul/`, `.sl/`, `.svn/`) and tool-state directories (`.conductor/`, `.entire/`, `.pi/`, `.worktrees/`). User config and project config exclusions are combined.

## Aliases

Command templates that run as `wt <name>`. See the [Extending Worktrunk guide](@/extending.md#aliases) for usage and flags.

```toml
[aliases]
deploy = "make deploy BRANCH={{ branch }}"
url = "echo http://localhost:{{ branch | hash_port }}"
```

Aliases defined here are shared with teammates. For personal aliases, use the [user config](@/config.md#aliases) `[aliases]` section instead.
<!-- PROJECT_CONFIG_END -->

# Shell Integration

Worktrunk needs shell integration to change directories when switching worktrees. Install with:

```console
$ wt config shell install
```

For manual setup, see `wt config shell init --help`.

Without shell integration, `wt switch` prints the target directory but cannot `cd` into it.

### First-run prompts

On first run without shell integration, Worktrunk offers to install it. Similarly, on first commit without LLM configuration, it offers to configure a detected tool (`claude`, `codex`). Declining sets `skip-shell-integration-prompt` or `skip-commit-generation-prompt` automatically.

# Other

## Environment variables

All user config options can be overridden with environment variables using the `WORKTRUNK_` prefix.

### Naming convention

Config keys use kebab-case (`worktree-path`), while env vars use SCREAMING_SNAKE_CASE (`WORKTRUNK_WORKTREE_PATH`). The conversion happens automatically.

For nested config sections, use double underscores to separate levels:

| Config | Environment Variable |
|--------|---------------------|
| `worktree-path` | `WORKTRUNK_WORKTREE_PATH` |
| `commit.generation.command` | `WORKTRUNK_COMMIT__GENERATION__COMMAND` |
| `commit.stage` | `WORKTRUNK_COMMIT__STAGE` |

Note the single underscore after `WORKTRUNK` and double underscores between nested keys.

### Example: CI/testing override

Override the LLM command in CI to use a mock:

```console
$ WORKTRUNK_COMMIT__GENERATION__COMMAND="echo 'test: automated commit'" wt merge
```

### Other environment variables

| Variable | Purpose |
|----------|---------|
| `WORKTRUNK_BIN` | Override binary path for shell wrappers; useful for testing dev builds |
| `WORKTRUNK_CONFIG_PATH` | Override user config file location |
| `WORKTRUNK_SYSTEM_CONFIG_PATH` | Override system config file location |
| `WORKTRUNK_PROJECT_CONFIG_PATH` | Override project config file location (defaults to `.config/wt.toml`) |
| `XDG_CONFIG_DIRS` | Colon-separated system config directories (default: `/etc/xdg`) |
| `WORKTRUNK_DIRECTIVE_CD_FILE` | Internal: set by shell wrappers. wt writes a raw path; the wrapper `cd`s to it |
| `WORKTRUNK_DIRECTIVE_EXEC_FILE` | Internal: set by shell wrappers. wt writes shell commands; the wrapper sources the file |
| `WORKTRUNK_SHELL` | Internal: set by shell wrappers to indicate shell type (e.g., `powershell`) |
| `WORKTRUNK_MAX_CONCURRENT_COMMANDS` | Max parallel git commands (default: 32). Lower if hitting file descriptor limits. |
| `NO_COLOR` | Disable colored output ([standard](https://no-color.org/)) |
| `CLICOLOR_FORCE` | Force colored output even when not a TTY |
<!-- subdoc: show -->
<!-- subdoc: approvals -->
<!-- subdoc: alias -->
<!-- subdoc: state -->"#)]
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },

    /// Run a custom `wt-<name>` command found on PATH.
    ///
    /// Captured by clap when the first positional argument doesn't match any
    /// built-in subcommand. The first element of the vec is the subcommand name;
    /// the rest are the arguments to pass through. See `commands::custom`.
    #[command(external_subcommand)]
    Custom(Vec<OsString>),
}
