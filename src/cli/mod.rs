mod config;
mod hook;
mod list;
mod step;

pub(crate) use config::{
    ApprovalsCommand, CacheAction, CiStatusAction, ConfigAliasCommand, ConfigCommand,
    ConfigPluginsClaudeCommand, ConfigPluginsCodexCommand, ConfigPluginsCommand,
    ConfigPluginsOpencodeCommand, ConfigShellCommand, DefaultBranchAction, GlobalFormatFlag,
    HintsAction, LogsAction, MarkerAction, PreviousBranchAction, StateCommand, StateWrite,
    VarsAction,
};
pub(crate) use hook::{HOOK_TYPE_NAMES, HookCommand, HookOptions, parse_hook_type};
pub(crate) use list::ListSubcommand;
pub(crate) use step::StepCommand;

use clap::builder::styling::{AnsiColor, Color, Styles};
use clap::{Args, Command, CommandFactory, Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::sync::OnceLock;

use crate::commands::Shell;

/// Reject an empty (or whitespace-only) branch-name argument.
///
/// The `value_parser` for every branch-name argument. Without it, `--branch=`
/// (or a bare empty positional) flows downstream as an empty branch name and
/// surfaces as a garbled diagnostic — `Branch  has no worktree` /
/// `wt switch ''`. Rejecting it at the parse boundary yields a clear usage
/// error instead.
pub(crate) fn non_empty_branch(s: &str) -> Result<String, String> {
    if s.trim().is_empty() {
        Err("branch name cannot be empty".to_string())
    } else {
        Ok(s.to_string())
    }
}

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
        // `option_env!`, not `env!`: when building from the crates.io package
        // archive there is no git worktree, so the build script can't set
        // `VERGEN_GIT_DESCRIBE` and the variable is undefined at compile time.
        // `env!` fails to compile in that case (see #3123); `option_env!`
        // yields `None` and we fall back to the cargo package version.
        resolve_version(
            option_env!("VERGEN_GIT_DESCRIBE"),
            env!("CARGO_PKG_VERSION"),
        )
    })
}

/// Choose between the git-describe version and the cargo package version.
///
/// Falls back to `cargo_version` when git describe is unavailable (`None`,
/// from a non-git build) or idempotent (vergen's placeholder for a
/// non-reproducible build).
fn resolve_version(git_version: Option<&str>, cargo_version: &str) -> String {
    match git_version {
        Some(git_version) if !git_version.contains("IDEMPOTENT") => git_version.to_string(),
        _ => cargo_version.to_string(),
    }
}

/// Output format for commands with text + JSON modes (e.g., `wt switch`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum SwitchFormat {
    Text,
    Json,
}

/// Output format for `wt list` and `wt config state get` (table or JSON).
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum OutputFormat {
    Table,
    Json,
}

/// Output format for `wt list statusline`, including the Claude Code mode.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum StatuslineFormat {
    Table,
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

    /// Override config with inline TOML, e.g. --config-set list.full=true (repeatable)
    #[arg(
        long = "config-set",
        global = true,
        value_name = "toml",
        display_order = 102,
        help_heading = "Global Options"
    )]
    pub config_override: Vec<String>,

    /// Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to apply the same level everywhere — including shell completion, which no flag can reach.
    #[arg(
        long,
        short = 'v',
        global = true,
        action = clap::ArgAction::Count,
        display_order = 103,
        help_heading = "Global Options"
    )]
    pub verbose: u8,

    /// Skip approval prompts
    #[arg(
        long,
        short = 'y',
        global = true,
        display_order = 104,
        help_heading = "Global Options"
    )]
    pub yes: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Shared `--no-hooks` / `--no-verify` flags for commands that resolve hook
/// skipping to a plain `bool` (`switch`, `remove`, `step commit`,
/// `step squash`).
///
/// `wt merge` does not flatten this struct: its hooks flag is tri-state
/// (`Option<bool>`, so config `[merge] verify` can still apply) and it carries
/// a positive `--verify` override. It declares its own flags but routes the
/// `--no-verify` deprecation through `crate::warn_no_verify_deprecated`, so the
/// warning text lives in exactly one place.
#[derive(Args)]
pub(crate) struct HookFlags {
    /// Skip hooks
    #[arg(long = "no-hooks", action = clap::ArgAction::SetFalse, default_value_t = true, help_heading = "Automation")]
    pub(crate) verify: bool,

    /// Skip hooks (deprecated alias for --no-hooks)
    #[arg(long = "no-verify", hide = true)]
    pub(crate) no_verify_deprecated: bool,
}

impl HookFlags {
    /// Resolve to the effective verify value, emitting the deprecation warning
    /// once if `--no-verify` was used.
    pub(crate) fn resolve(&self) -> bool {
        if self.no_verify_deprecated {
            crate::warn_no_verify_deprecated();
            false
        } else {
            self.verify
        }
    }
}

#[derive(Args)]
pub(crate) struct SwitchArgs {
    /// Branch name, shortcut, or PR/MR URL
    ///
    /// Opens interactive picker if omitted.
    /// Shortcuts: `^` (default branch), `-` (previous), `@` (current), `pr:{N}` (GitHub PR), `mr:{N}` (GitLab MR)
    #[arg(add = crate::completion::worktree_branch_completer(), value_parser = crate::cli::non_empty_branch)]
    pub(crate) branch: Option<String>,

    /// Include branches without worktrees
    #[arg(long, help_heading = "Picker Options", conflicts_with_all = ["create", "base", "execute", "execute_args", "clobber"])]
    pub(crate) branches: bool,

    /// Include remote branches
    #[arg(long, help_heading = "Picker Options", conflicts_with_all = ["create", "base", "execute", "execute_args", "clobber"])]
    pub(crate) remotes: bool,

    /// Include open PRs/MRs
    #[arg(long, help_heading = "Picker Options", conflicts_with_all = ["create", "base", "execute", "execute_args", "clobber"])]
    pub(crate) prs: bool,

    /// Create a new branch
    #[arg(short = 'c', long, requires = "branch")]
    pub(crate) create: bool,

    /// Base branch
    ///
    /// Defaults to default branch. Supports the same shortcuts as the branch
    /// argument: `^`, `@`, `-`, `pr:{N}`, `mr:{N}`.
    #[arg(short = 'b', long, requires = "branch", add = crate::completion::branch_value_completer(), value_parser = crate::cli::non_empty_branch)]
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
    /// Template example: `-x code -- '{{ worktree_path }}'` opens VS Code
    /// at the worktree, `-x tmux -- new -s '{{ branch | sanitize }}'` starts
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
    #[arg(long, overrides_with = "cd")]
    pub(crate) no_cd: bool,

    /// Change directory after switching
    #[arg(long, overrides_with = "no_cd", hide = true)]
    pub(crate) cd: bool,

    #[command(flatten)]
    pub(crate) hooks: HookFlags,

    /// Output format
    ///
    /// JSON prints structured result to stdout. Designed for tool
    /// integration (e.g., Claude Code WorktreeCreate hooks).
    #[arg(long, default_value = "text", help_heading = "Automation")]
    pub(crate) format: SwitchFormat,
}

#[derive(Args)]
pub(crate) struct ListArgs {
    #[command(subcommand)]
    pub(crate) subcommand: Option<ListSubcommand>,

    /// Output format
    #[arg(long, value_enum, default_value = "table")]
    pub(crate) format: OutputFormat,

    /// Include branches without worktrees
    #[arg(long)]
    pub(crate) branches: bool,

    /// Include remote branches
    #[arg(long)]
    pub(crate) remotes: bool,

    /// Show CI status and LLM summaries
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
    /// Branch name or worktree path [default: current]
    #[arg(add = crate::completion::local_branches_completer(), value_parser = crate::cli::non_empty_branch)]
    pub(crate) branches: Vec<String>,

    /// Keep branch after removal
    #[arg(long = "no-delete-branch", overrides_with = "delete_branch")]
    pub(crate) no_delete_branch: bool,

    /// Delete branch after removal (overrides config `[remove] delete-branch = false`)
    #[arg(
        long = "delete-branch",
        overrides_with = "no_delete_branch",
        hide = true
    )]
    pub(crate) delete_branch: bool,

    /// Delete unmerged branches
    #[arg(short = 'D', long = "force-delete")]
    pub(crate) force_delete: bool,

    /// Run removal in foreground (block until complete)
    #[arg(long)]
    pub(crate) foreground: bool,

    /// Kill processes started in the worktree \[experimental\]
    ///
    /// Before removal, terminate processes whose working directory is under
    /// the worktree — dev servers, watchers, language servers. Processes
    /// holding a controlling terminal (interactive shells, terminal editors)
    /// are left alone. Unix only.
    #[arg(long)]
    pub(crate) reap: bool,

    #[command(flatten)]
    pub(crate) hooks: HookFlags,

    /// Force worktree removal
    ///
    /// Remove a dirty worktree, including staged, modified, and untracked
    /// files. Without this flag, removal fails if the worktree has any
    /// uncommitted changes.
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
    #[arg(add = crate::completion::branch_value_completer(), value_parser = crate::cli::non_empty_branch)]
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
$ wt switch https://github.com/owner/repo/pull/123   # ...or paste the PR's URL
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

When called without arguments, `wt switch` opens an interactive picker to browse and select worktrees with live preview. The candidate set widens with `--branches` (local branches without worktrees), `--remotes` (remote branches), and `--prs` (open PRs/MRs — see below).

The CI column shows each row's PR/MR CI and review status, the same as [`wt list --full`](@/list.md).

<!-- demo: wt-switch-picker.gif 1600x800 -->
**Keybindings:**

| Key | Action |
|-----|--------|
| `↑`/`↓` | Navigate worktree list |
| (type) | Filter worktrees |
| `Enter` | Switch to selected worktree |
| `Alt-c` | Create new worktree named as entered text |
| `Alt-x` | Remove selected worktree/branch |
| `Alt-y` | Copy selected branch name to the clipboard |
| `Alt-o` | Open the selected row's PR/MR URL in the browser |
| `Alt-r` | Refresh the list (pick up worktrees created elsewhere) |
| `Esc` | Cancel |
| `Alt-1`–`Alt-7` | Jump to a preview tab |
| `Tab`/`Shift-Tab` | Cycle preview tabs forward/backward |
| `Alt-p` | Toggle preview panel |
| `Ctrl-u`/`Ctrl-d` | Scroll preview up/down |

`Alt-o` is a no-op on a row with no PR/MR (or whose status hasn't loaded yet).

`Alt-x` is a no-op on the current worktree (the `@` row) — removing the worktree in use would have to switch elsewhere first, so switch away and remove it from there.

Each row filters by its branch, path, and — when it has a PR/MR — the PR/MR's number, title, and author, the same fields whether the PR is checked out (a worktree row) or listed via `--prs`. Plain digits go to the filter, so a number can be typed directly and the preview tabs move to `Alt`.

Typing a gutter sigil filters by row kind: `+` narrows to linked worktrees and `@` to the current worktree. The other sigils don't filter cleanly — `^` and `|` are skim's prefix-anchor and OR query operators (so `^` matches every row and `|` none), and `/` matches most rows because every worktree path contains it.

**Preview tabs:**

1. **HEAD±** — Diff of uncommitted changes
2. **log** — Recent commits; commits already on the default branch have dimmed hashes
3. **main…±** — Diff of changes since the merge-base with the default branch
4. **remote⇅** — Ahead/behind diff vs upstream tracking branch
5. **summary** — LLM-generated branch summary; requires `[list] summary = true` and [`commit.generation`](@/config.md#commit)
6. **pr** — The selected row's PR/MR, for any row whose branch has one
7. **comments** — The PR/MR's comment thread, fetched from the forge on `--prs` rows

On narrow previews the tab bar compacts to digits — only the active tab keeps its label — so every `Alt-N` accelerator stays visible.

**Pager configuration:** The preview panel pipes diff output through git's pager. Override in user config:

```toml
[switch.picker]
pager = "delta --paging=never --width=$COLUMNS"
```

## Pull requests and merge requests

The `pr:<number>` / `mr:<number>` shortcut and the PR/MR's web URL both resolve to its branch. For same-repo PRs/MRs, worktrunk switches to the branch directly. For fork PRs/MRs, it fetches the ref (`refs/pull/N/head` or `refs/merge-requests/N/head`) and configures `pushRemote` to the fork URL.

```console
$ wt switch pr:101                                  # GitHub PR #101
$ wt switch https://github.com/owner/repo/pull/101  # ...the same PR, by URL
$ wt switch mr:101                                  # GitLab MR !101
$ wt switch https://gitlab.com/owner/repo/-/merge_requests/101  # ...the same MR, by URL
$ wt switch --prs                                   # Browse open PRs/MRs in the picker
```

Both work anywhere a branch is accepted, including `--base`. The `--create` flag cannot be used with a PR/MR reference since the branch already exists.

If the PR or MR is on a fork, the local branch uses its branch name directly, so `git push` works normally. A pre-existing local branch with that name tracking something else requires renaming first.

The `--prs` flag adds the repository's open PRs (GitHub) or MRs (GitLab) to the interactive picker — only the ones not already there: a PR whose branch is already shown (as a worktree, or a local or remote branch) isn't listed twice, so `--prs` only adds the rest and the two pickers differ solely by those extra rows. Each added row resolves to the same `pr:`/`mr:` shortcut, so selecting one fetches the ref and switches to its branch. A `--prs` row has no local worktree, so its `pr` and `comments` preview tabs load the PR/MR's metadata and comments from the forge in the background. The `log` tab uses a local `git log` — graph and merge-base dimming included — whenever the head commit is already in the object store (a same-repo PR off a fetched remote), falling back to a flat forge-fetched commit list otherwise.

Requires `gh` (GitHub), `glab` (GitLab), or an equivalent CLI installed and authenticated; see [forge platform](@/config.md#forge-platform) for Gitea, Azure DevOps, and other supported platforms.

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

`--full` adds the two columns that reach off-machine: [CI status](#ci-status) (GitHub/GitLab pipeline pass/fail, over the network) and [LLM-generated summaries](#llm-summaries) of each branch's changes. The `main…±` line diffs are local git, so they show by default.

## Examples

List all worktrees:

<!-- wt list -->
```console
$ wt list
  Branch       Status        HEAD±    main↕     main…±  Remote⇅  Commit    Age   Message
@ feature-api  +   ↕⇡     +54   -5   ↑4  ↓1  +234  -24   ⇡3      6814f02a  30m   Add API tests
^ main             ^⇅                                    ⇡1  ⇣1  41ee0834  4d    Merge fix-auth:…
+ fix-auth         ↕|                ↑2  ↓1   +25  -11     |     b772e68b  5h    Add secure token…
+ fix-typos        _|                                      |     41ee0834  4d    Merge fix-auth:…

○ Showing 4 worktrees, 1 with changes, 2 ahead, 1 column hidden
```

Include CI status and LLM summaries:

<!-- wt list --full -->
```console
$ wt list --full
  Branch       Status        HEAD±    main↕     main…±  Summary                                                Remote⇅  CI    Commit
@ feature-api  +   ↕⇡     +54   -5   ↑4  ↓1  +234  -24  Refactor API to REST architecture with middleware       ⇡3      #412  6814f02a
^ main             ^⇅                                                                                           ⇡1  ⇣1  #     41ee0834
+ fix-auth         ↕|                ↑2  ↓1   +25  -11  Harden auth with constant-time token validation           |     #408  b772e68b
+ fix-typos        _|                                                                                             |     #410  41ee0834

○ Showing 4 worktrees, 1 with changes, 2 ahead, 3 columns hidden
```

Include branches that don't have worktrees:

<!-- wt list --branches --full -->
```console
$ wt list --branches --full
  Branch       Status        HEAD±    main↕     main…±  Summary                                                Remote⇅  CI    Commit
@ feature-api  +   ↕⇡     +54   -5   ↑4  ↓1  +234  -24  Refactor API to REST architecture with middleware       ⇡3      #412  6814f02a
^ main             ^⇅                                                                                           ⇡1  ⇣1  #     41ee0834
+ fix-auth         ↕|                ↑2  ↓1   +25  -11  Harden auth with constant-time token validation           |     #408  b772e68b
+ fix-typos        _|                                                                                             |     #410  41ee0834
/ exp             /↕                 ↑2  ↓1  +137       Explore GraphQL schema and resolvers                                  96379229
/ wip             /↕                 ↑1  ↓1   +33       Start API documentation                                               b40716dc

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
| main…± | Line diffs since the merge-base (three-dot) with the default branch |
| Summary | LLM-generated branch summary; requires `--full`, `summary = true`, and [`commit.generation`](@/config.md#commit) [experimental] |
| Remote⇅ | Commits ahead/behind tracking branch |
| CI | PR/MR number colored by pipeline status; `--full` only |
| Path | Worktree directory |
| URL | Dev server URL from project config; dimmed if port is not listening |
| *(custom)* | User-defined [custom columns](#custom-columns) from `[list.custom-columns]` user config [experimental] |
| Commit | Short hash (8 chars) |
| Age | Time since last commit |
| Message | Last commit message (truncated) |

The `main` header label is used regardless of the default branch's actual name.

`main↕` and `main…±` measure against the default branch's upstream tip when the local copy lags it — so in a fork whose local `main` trails `origin/main`, a branch reads as ahead of the real mainline, not of a stale local checkout. The `↑`/`↓`/`↕` Status symbols derive from these counts, so they track the upstream tip too.

### Gutter

The leftmost column marks each row by physical presence, from most present to least:

| Symbol | Meaning |
|--------|---------|
| `@` | Current worktree |
| `^` | Primary worktree (the repo's home worktree) |
| `+` | Other worktree |
| `/` | Local branch without a worktree (`--branches`) |
| `\|` | Remote branch, not present locally until fetched (`--remotes`) |

### CI status

The CI column shows the branch's open PR/MR — `#3035` on GitHub, Gitea, and Azure DevOps, `!3035` on GitLab — colored by pipeline status, or a bare `#` when no number is available (e.g. branch workflows without a PR/MR). One color folds two JSON fields: green/blue/red/yellow/gray are `ci.status`; magenta/cyan are `ci.review_state`. The `Value` column is the matching JSON string from `--format=json`:

| Indicator | Value | Meaning |
|-----------|-------|---------|
| `#` green | `"passed"` | All checks passed |
| `#` blue | `"running"` | Checks in progress |
| `#` red | `"failed"` | One or more checks failed |
| `#` yellow | `"conflicts"` | Merge conflicts with the target branch |
| `#` gray | `"no-ci"` | No PR/MR, or no checks configured |
| `⚠` yellow | `"error"` | CI status could not be fetched (rate limit, network, etc.) |
| `#` magenta | `"changes_requested"` | A reviewer requested changes |
| `#` cyan | `"pending"` | A review is required (e.g. branch protection) but not yet given |
| (blank) | `ci` absent | No upstream, or no PR/MR and no branch workflow |

The two remaining `ci.review_state` values have no indicator of their own: `"draft"` only dims the cell and `"approved"` leaves the color unchanged.

Color precedence resolves the fold: changes-requested (magenta) outranks running checks — waiting can't clear it — while an outstanding required review (cyan) only recolors an otherwise green or quiet branch. Cool colors mean waiting, warm colors mean act. An approved PR, or one with no review signal at all (no required reviewers and no reviews), keeps its plain `ci.status` color — `ci.review_state` is then `"approved"` or absent, respectively. GitLab MR data carries only `"pending"` and `"draft"` — no approved or changes-requested signal.

CI cells are clickable links to the PR or pipeline page, and appear dimmed for a draft PR/MR (`"draft"`) or when unpushed local changes make the status stale (`ci.stale`). PRs/MRs are checked first, then branch workflows/pipelines for branches with an upstream. Local-only branches show blank; remote-only branches — visible with `--remotes` — get CI status detection. Results are cached for 30-60 seconds; use `wt config state` to view or clear.

### LLM summaries [experimental]

Reuses the [`commit.generation`](@/config.md#commit) command — the same LLM that generates commit messages. Enable with `summary = true` in `[list]` config; requires `--full`. Results are cached until the branch's diff changes.

### Custom columns [experimental]

Each `[list.custom-columns]` entry in user config adds a column: the key is the header, the template renders each row's cell. Templates read two per-branch namespaces — `{{ vars.* }}`, stored with [`wt config state vars set`](@/config.md#wt-config-state-vars), and `{{ git.branch.* }}`, the branch's own git config under `branch.<name>.*` (a `jira` key you set yourself, or the git-native `description`) — useful for tracking what each of many (often agent-driven) branches is for:

```toml
[list.custom-columns.Ticket]
template = "{{ vars.ticket }}"
```

A column that renders empty for every row is dropped from the table. Templates, widths, and drop priority: [custom columns config](@/config.md#custom-columns).

## Status symbols

The Status column packs several subcolumns, left to right, each mapping to a field in `--format=json`. Working-tree flags are independent and co-occur — any combination shows at once. The other subcolumns are mutually exclusive: each shows a single symbol, the highest-priority state in top-to-bottom table order, and is blank when nothing applies.

### Working tree

Independent flags from `git status`; several can show at once (e.g. `+!?`). Each maps to a boolean in the `working_tree` object:

| Symbol | working_tree | Meaning |
|--------|--------------|---------|
| `+` | `staged` | Staged files |
| `!` | `modified` | Modified files (unstaged) |
| `?` | `untracked` | Untracked files |

`working_tree` also reports `renamed` and `deleted`, which have no dedicated symbol in the column.

### Worktree

An in-progress git operation, a worktree-location attribute, or a branch with no worktree. One symbol shows, highest priority first (`✘ > ⤴ > ⤵ > ⚑ > ⊟ > ⊞ > /`):

| Symbol | JSON | Meaning |
|--------|------|---------|
| `✘` | `operation_state` `"conflicts"` | Merge conflicts |
| `⤴` | `operation_state` `"rebase"` | Rebase in progress |
| `⤵` | `operation_state` `"merge"` | Merge in progress |
| `⚑` | `worktree.state` `"branch_worktree_mismatch"` | Branch name doesn't match the worktree path |
| `⊟` | `worktree.state` `"prunable"` | Prunable (worktree directory missing) |
| `⊞` | `worktree.state` `"locked"` | Locked worktree |
| `/` | `kind` `"branch"` | Branch without a worktree (no `worktree` object) |

### Default branch

The single highest-priority state describing the branch's relation to the default branch; blank when none applies (a normal up-to-date branch). Each symbol is one `main_state` value:

| Symbol | main_state | Meaning |
|--------|------------|---------|
| `^` | `"is_main"` | The main worktree (the repo's home worktree) |
| `∅` | `"orphan"` | No common ancestor with the default branch |
| `_` | `"empty"` | Same commit as the default branch, working tree clean — safe to remove; row dimmed |
| `⊂` | `"integrated"` | Content [integrated](@/remove.md#branch-cleanup) into the default branch or merge target via different history; the matching check is in `integration_reason`; row dimmed |
| `✗` | `"would_conflict"` | Merging into the default branch would conflict (simulated with `git merge-tree`) and the branch isn't already integrated; with `--full`, the check includes uncommitted changes |
| `–` | `"same_commit"` | Same commit as the default branch, but with uncommitted changes |
| `↕` | `"diverged"` | Both ahead of and behind the default branch |
| `↑` | `"ahead"` | Has commits the default branch doesn't |
| `↓` | `"behind"` | Missing commits the default branch has |

Rows are dimmed when [safe to delete](@/remove.md#branch-cleanup) — `_` (`"empty"`) or `⊂` (`"integrated"`).

### Remote

Relation to the tracking branch, derived from the `remote.ahead` / `remote.behind` counts; blank when there is no upstream:

| Symbol | remote | Meaning |
|--------|--------|---------|
| `\|` | `ahead` 0, `behind` 0 | In sync with remote |
| `⇡` | `ahead` > 0 | Ahead of remote |
| `⇣` | `behind` > 0 | Behind remote |
| `⇅` | `ahead` > 0, `behind` > 0 | Diverged from remote |

### Placeholder symbols

These appear across all columns while the table is loading:

| Symbol | Meaning |
|--------|---------|
| `·` | Data is loading, or collection timed out / branch too stale |

---

## JSON output

`--format=json` emits structured data in one of two schemas while the format
migrates: `[list] json-schema = 2` selects the envelope format below, `= 1`
the original bare-array format. Unset emits schema 1 with a warning
(`wt config update` pins `= 1`); a future release flips the default to
schema 2 and later removes schema 1.

### Schema 2

One envelope object. Items carry independent facts; rendered strings
(including the collapsed Status value) live under `display`:

```json
{
  "schema": 2,
  "repo": {
    "default_branch": "main",
    "forge": {"url": "https://github.com/org/repo", "provider": "github",
              "host": "github.com", "owner": "org", "name": "repo", "remote": "origin"}
  },
  "collected": {"ci": false, "summary": false},
  "items": [
    {
      "branch": "feature",
      "head": {"sha": "05a4a45d…", "short_sha": "05a4a45", "subject": "Add login page",
               "committed_at": "2025-01-01T08:00:00Z"},
      "worktree": {"path": "/home/user/repo.feature", "main": false, "current": true,
                   "previous": false, "detached": false, "branch_mismatch": false,
                   "changes": {"staged": false, "modified": true, "untracked": false,
                               "renamed": false, "deleted": false, "conflicted": false,
                               "diff": {"added": 10, "deleted": 2}}},
      "default_branch": {"ahead": 3, "behind": 1, "diff": {"added": 50, "deleted": 20},
                         "orphan": false, "integration": null, "merge_conflicts": false},
      "upstream": {"remote": "origin", "branch": "feature", "ahead": 0, "behind": 2},
      "display": {"state": "diverged", "symbols": "!↕", "statusline": "feature …"}
    }
  ]
}
```

How "no value" reads:

- **Absent** — nothing to report: not applicable (`worktree` on a branch-only
  row), not requested this run (the envelope's `collected` records what was),
  or determined-empty (no PR, no lock, not integrated).
- **`null`** — requested but not determined: a task timed out, the branch was
  too stale for the expensive checks, or a forge fetch failed. This is the
  JSON form of the table's `·` placeholder.

jq treats absent and `null` identically in path expressions, so filters need
no null checks; `has()` distinguishes the two when it matters.

Item fields:

| Field | Description |
|-------|-------------|
| `branch` | Branch name; null for a detached-HEAD worktree. Remote rows carry the bare name with the remote in `remote` |
| `remote` | Remote name, present only on remote-only branch rows |
| `head` | `{sha, short_sha, subject, committed_at}`; null for unborn branches. `committed_at` is RFC 3339 UTC |
| `worktree` | `{path, main, current, previous, detached, locked, prunable, branch_mismatch, operation, changes}`; absent on branch-only rows. `locked`/`prunable` are `{reason}` objects and can co-occur; `operation` is `"rebase"` or `"merge"`; `changes` holds the five working-tree flags plus `conflicted` and `diff {added, deleted}` |
| `default_branch` | Relation to the default branch: `{ahead, behind, diff, orphan, integration, merge_conflicts}`; absent on the default branch itself. `integration.reason` is one of `same_commit`, `ancestor`, `no_added_changes`, `trees_match`, `merge_adds_nothing`, `patch_id_match`; a dirty tree skips the checks, leaving `integration` null |
| `upstream` | Tracking branch: `{remote, branch, ahead, behind}`; absent when none is configured |
| `pr` | Open PR/MR: `{number, url, review, mergeable, repo}`; collected with `--full` or a listed `ci` column. `review` uses the schema 1 `ci.review_state` vocabulary; `mergeable` is false when the forge reports conflicts, null otherwise |
| `checks` | CI pipeline: `{status, source, stale}`; `status` is `passed`, `running`, or `failed` — null when a conflicts report masks it |
| `dev_server` | `{url, listening}` from the project's `list.url` template |
| `summary` | LLM branch summary (requires `[list] summary = true`) |
| `vars` | Per-branch variables from [`wt config state vars`](@/config.md#wt-config-state-vars) |
| `display` | Rendered strings: `state` (schema 1's `main_state` vocabulary), `symbols`, `statusline` (with ANSI colors), `columns` (custom-column cells keyed by header) |

Schema 1 names map directly: `commit` → `head`, `working_tree` →
`worktree.changes`, `main` + `main_state` → `default_branch` +
`display.state`, `remote` → `upstream`, `ci` → `pr` + `checks`, `url` +
`url_active` → `dev_server`, `statusline`/`symbols`/`columns` → `display.*`,
and the per-item `repo` moves to the envelope's `repo.forge`.

```console
# Current worktree path (for scripts)
$ wt list --format=json | jq -r '.items[] | select(.worktree.current) | .worktree.path'

# Branches with uncommitted changes
$ wt list --format=json | jq '.items[] | select(.worktree.changes.modified)'

# Integrated branches (safe to remove)
$ wt list --format=json | jq '.items[] | select(.display.state == "integrated" or .display.state == "empty") | .branch'

# Worktrees ahead of upstream (needs pushing)
$ wt list --format=json | jq '.items[] | select(.upstream.ahead > 0) | .branch'
```

### Schema 1

The original bare-array format, and the default while unset:

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
| `operation_state` | string | `"conflicts"`, `"rebase"`, or `"merge"` (see [Worktree](#worktree)); absent when clean |
| `main` | object | Relationship to the default branch (see below); absent when is_main |
| `remote` | object | Tracking branch info (see below); absent when no tracking |
| `worktree` | object | Worktree metadata (see below) |
| `is_main` | boolean | Is the main worktree |
| `is_current` | boolean | Is the current worktree |
| `is_previous` | boolean | Previous worktree from wt switch |
| `ci` | object | CI status (see below); `--full` only, then absent when no PR/MR or branch workflow |
| `repo_url` | string | Repository web URL derived from the primary remote; absent when the remote URL cannot be parsed |
| `repo` | object | Structured repository metadata (see below); includes `remote` |
| `url` | string | Dev server URL from project config; absent when not configured |
| `url_active` | boolean | Whether the URL's port is listening; absent when not configured |
| `summary` | string | LLM-generated branch summary; `--full` only, then absent when not configured or no summary |
| `statusline` | string | Pre-formatted status with ANSI colors |
| `symbols` | string | Raw status symbols without colors (e.g., `"!?↓"`) |
| `vars` | object | Per-branch variables from [`wt config state vars`](@/config.md#wt-config-state-vars) (absent when empty) |
| `columns` | object | Rendered [custom column](#custom-columns) values keyed by header; empty cells omitted (absent when none configured) |

### Commit object

| Field | Type | Description |
|-------|------|-------------|
| `sha` | string | Full commit SHA (40 chars) |
| `short_sha` | string | Short commit SHA, abbreviated per `core.abbrev` (auto-extends for ambiguous prefixes) |
| `message` | string | Commit message (first line) |
| `timestamp` | number | Unix timestamp |

### working_tree object

The five change flags map to the [Working tree](#working-tree) symbols (`renamed` and `deleted` have none of their own):

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

`ahead` / `behind` drive the [Remote](#remote) divergence symbol:

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Remote name (e.g., `"origin"`) |
| `branch` | string | Remote branch name |
| `ahead` | number | Commits ahead of remote |
| `behind` | number | Commits behind remote |

### worktree object

Present only for worktree-kind items. `state` is the worktree-location attribute — see [Worktree](#worktree) for its symbols:

| Field | Type | Description |
|-------|------|-------------|
| `state` | string | `"branch_worktree_mismatch"`, `"prunable"`, or `"locked"` (absent when normal) |
| `reason` | string | Reason for locked/prunable state |
| `detached` | boolean | HEAD is detached |

### ci object

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | CI status (see below) |
| `source` | string | `"pr"` (PR/MR) or `"branch"` (branch workflow) |
| `number` | integer | PR/MR number; absent for branch workflows |
| `stale` | boolean | Local HEAD differs from remote (unpushed changes) |
| `url` | string | URL to the PR/MR page |
| `repo_url` | string | Web URL of the repo the PR/MR targets (the upstream for fork PRs); absent when `url` is absent or unrecognized |
| `repo` | object | Structured metadata for the repository the PR/MR targets; never includes `remote` |
| `review_state` | string | Review state (see below); absent when the forge reports no review signal |

### repo object

Top-level `repo` describes the local checkout's repository as derived from the primary remote. `ci.repo` describes the repository targeted by the PR/MR URL in `ci.url` (for fork PRs, this is the upstream target). Existing `repo_url` and `ci.repo_url` fields remain available and carry the same URL as `repo.url` / `ci.repo.url`.

| Field | Type | Description |
|-------|------|-------------|
| `url` | string | Repository web URL |
| `provider` | string | `"github"`, `"gitlab"`, `"gitea"`, `"azure-devops"`, or `"unknown"` |
| `host` | string | Repository web host |
| `owner` | string | Owner, organization, or namespace path |
| `name` | string | Repository name |
| `project` | string | Azure DevOps project name; absent for other providers |
| `remote` | string | Local remote name used for top-level repo metadata; absent from `ci.repo` |

### main_state values

The single highest-priority state describing the branch's relation to the default branch; absent when none applies (a normal up-to-date branch). Each value is one Default-branch symbol — see [Default branch](#default-branch) for the symbol and the full meaning of each value (`"is_main"`, `"orphan"`, `"empty"`, `"integrated"`, `"would_conflict"`, `"same_commit"`, `"diverged"`, `"ahead"`, `"behind"`).

### integration_reason values

Set only when `main_state == "integrated"` (the `⊂` symbol), recording which check matched. Checks run cheapest-first and the first match wins. JSON-only — every reason renders as the same `⊂`:

| Value | Meaning |
|-------|---------|
| `"ancestor"` | Branch HEAD is an ancestor of the default branch, which has moved past it |
| `"no-added-changes"` | The three-dot diff (`main...branch`) is empty — no file changes beyond the merge-base |
| `"trees-match"` | Different history, but the branch's tree is identical to the default branch's |
| `"merge-adds-nothing"` | The branch has changes, but merging them leaves the default branch's tree unchanged (e.g. a squash merge where the target advanced on other files) |
| `"patch-id-match"` | The branch's squashed diff matches a single commit on the default branch (e.g. a GitHub/GitLab squash merge) |

### ci.status and ci.review_state values

The [CI status](#ci-status) section above is the single source for both fields: the table maps each colored value, and the notes below it cover `"draft"` and `"approved"`. `ci.status` is one of `"passed"`, `"running"`, `"failed"`, `"conflicts"`, `"no-ci"`, `"error"`; `ci.review_state` is one of `"changes_requested"`, `"pending"`, `"draft"`, `"approved"`, absent when the forge reports no review signal. The vocabulary matches Claude Code's statusline `pr.review_state` field.

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

The default-branch walk is capped so a single check stays fast; a squash merge with hundreds of commits landed since the merge point falls outside the cap and needs `-D` to remove.

The 'same commit' check uses the local default branch; for other checks, 'target' means the default branch, or its upstream (e.g., `origin/main`) when strictly ahead.

Branches matching these conditions and with empty working trees are dimmed in `wt list` as safe to delete.

## Force flags

Worktrunk has two force flags for different situations:

| Flag | Scope | When to use |
|------|-------|-------------|
| `--force` (`-f`) | Worktree | Worktree has uncommitted changes |
| `--force-delete` (`-D`) | Branch | Branch has unmerged commits |

```console
$ wt remove feature --force       # Remove dirty worktree
$ wt remove feature -D            # Delete unmerged branch
$ wt remove feature --force -D    # Both
```

Use `--no-delete-branch` to keep the branch regardless of merge status.

## Background removal

Removal runs in the background by default — the command returns immediately. The worktree is renamed into `.git/wt/trash/` (instant same-filesystem rename), git metadata is pruned, the branch is deleted, and a detached `rm -rf` finishes cleanup. Cross-filesystem worktrees fall back to `git worktree remove`. Logs: `.git/wt/logs/{branch}/internal/remove.log`. Use `--foreground` to run in the foreground.

After each `wt remove`, entries in `.git/wt/trash/` older than 24 hours are swept by a detached `rm -rf` — eventual cleanup for directories orphaned when a previous background removal was interrupted (SIGKILL, reboot, disk full).

## Reaping processes [experimental]

`--reap` terminates processes left running in the worktree before it is removed — a `post-start` dev server, a file watcher, a language server — freeing the ports and file handles they hold. Processes are discovered by working directory: any process whose current directory is at or under the worktree path (`SIGTERM`, then `SIGKILL` for survivors).

```console
$ wt remove --reap feature
◎ Reaping 2 processes under feature worktree
   ┃ 51234 node
   ┃ 51240 esbuild
✓ Reaped 2 processes
◎ Removing feature worktree & branch in background (same commit as main, _)
```

To avoid killing work the user did not mean to kill, two guards keep `--reap` conservative:

- **Interactive processes are spared.** A process holding a controlling terminal — an interactive shell, or a terminal editor such as `vim` with unsaved buffers — is never reaped. Only detached processes remain candidates.
- **Discovery is by working directory only.** A process that started in the worktree and later changed directory, or a daemon that reparented to `init`, no longer reports a directory under the worktree and is not found. To reliably reap those, launch them with [`wt step tether`](@/step.md#wt-step-tether), which kills the whole process group when the worktree is removed.

Reaping runs before the worktree directory is touched, so it is independent of foreground/background removal and the `--force` flag. Unix only; on Windows `--reap` is rejected.

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
- [`tether`](#wt-step-tether) — [experimental] Run a command; kill its whole process tree when its worktree is removed
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
<!-- subdoc: relocate -->
<!-- subdoc: tether -->"#
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
| **create** | `pre-start` | `post-start` |
| **commit** | `pre-commit` | `post-commit` |
| **merge** | `pre-merge` | `post-merge` |
| **remove** | `pre-remove` | `post-remove` |

`pre-*` hooks block — failure aborts the operation. `post-*` hooks run in the background with output logged (use [`wt config state logs`](@/config.md#wt-config-state-logs) to find and manage log files). Use `-v` to see the template variables for background hooks; `wt hook <type> --dry-run` previews the commands.

The most common creation hook is `post-start` — it runs background tasks (dev servers, file copying, builds) without blocking worktree creation. Prefer `post-start` over `pre-start` unless a later step needs the work completed first.

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

During `wt merge`, hooks run in this order: pre-commit → post-commit → pre-merge → pre-remove → post-remove + post-merge. See [`wt merge`](@/merge.md#pipeline) for the complete pipeline.

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
- Declining skips every project command for that operation — including any already approved — and continues without them; saved approvals are unaffected
- Use `--yes` to bypass prompts — useful for CI and automation
- Use `--no-hooks` to skip hooks

Manage approvals with `wt config approvals add` and `wt config approvals clear`.

# Configuration

Hooks can be defined in project config (`.config/wt.toml`) or user config (`~/.config/worktrunk/config.toml`). Both use the same format. The project config is read from the worktree the command ran in.

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

Templates are syntax-checked before the pipeline starts and rendered as each step runs, so a step can store [per-branch vars](@/config.md#wt-config-state-vars) that later steps read via `{{ vars.<key> }}`.

Most hooks don't need `[[hook]]` blocks. Reach for them when there's a dependency chain — typically setup that must complete before later steps, like installing dependencies before running a build and dev server concurrently.

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
|           | `{{ short_commit }}`          | Branch HEAD SHA, abbreviated per `core.abbrev` |
|           | `{{ upstream }}`              | Branch upstream (if tracking a remote) |
| operation | `{{ base }}`                  | Base branch name (switch/create only) |
|           | `{{ base_worktree_path }}`    | Base worktree path |
|           | `{{ target }}`                | Target branch name |
|           | `{{ target_worktree_path }}`  | Target worktree path (when target has a worktree) |
|           | `{{ pr_number }}`             | PR/MR number (post-switch, pre-start, post-start; when creating via `pr:N` / `mr:N`) |
|           | `{{ pr_url }}`                | PR/MR web URL (post-switch, pre-start, post-start; when creating via `pr:N` / `mr:N`) |
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

The `repo` variables (`repo`, `repo_path`, `owner`, `primary_worktree_path`, `default_branch`, `remote`, `remote_url`) are constant across the whole repository — `default_branch` is the same in every worktree. The `active` variables (`branch`, `worktree_path`, `worktree_name`, `commit`, `short_commit`, `upstream`) vary per worktree.

Bare variables (`branch`, `worktree_path`, `commit`) refer to the branch the operation acts on: the destination for switch/create, the source for merge/remove. `base` and `target` give the other side:

| Operation | Bare vars | `base` | `target` |
|-----------|-----------|--------|----------|
| switch/create | destination | where you came from | = bare vars |
| commit (during merge/squash) | worktree being squashed | = bare vars | integration target |
| merge | feature being merged | = bare vars | merge target |
| remove | branch being removed | = bare vars | where you end up |

All hooks share the same perspective — `{{ branch | hash_port }}` produces the same port in `post-start` and `post-remove`.

`cwd` is the worktree root where the hook command runs. It equals `worktree_path` except in three cases:

- `pre-switch`: hook runs in the source worktree; `worktree_path` is the destination
- `post-remove`: the active worktree is gone, so the hook runs in the primary worktree
- `post-merge` with removal: the active worktree is gone, so the hook runs in the target worktree

Undefined variables error — use conditionals or defaults for optional behavior:

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
| `sanitize_db` | `{{ branch \| sanitize_db }}` | Database-safe identifier with hash suffix (`[a-z0-9_]`, max 48 chars) |
| `sanitize_hash` | `{{ branch \| sanitize_hash }}` | Filesystem-safe name with hash suffix for uniqueness |
| `hash` | `{{ branch \| hash }}` | 3-character base36 digest of the input |
| `hash_port` | `{{ branch \| hash_port }}` | Hash to port 10000-19999 |
| `dirname` | `{{ repo_path \| dirname }}` | Strip the last path component (`/a/b/c` → `/a/b`) |
| `basename` | `{{ repo_path \| basename }}` | Keep only the last path component (`/a/b/c` → `c`) |
| `codename(n)` | `{{ branch \| codename(2) }}` | Deterministic friendly words |

The `sanitize_db` filter produces database-safe identifiers — lowercase alphanumeric and underscores, no leading digits, with a 3-character hash suffix to avoid collisions and reserved words. The `sanitize_hash` filter produces a filesystem-safe name and appends a 3-character hash suffix when sanitization changed the input, so distinct originals never collide — already-safe names pass through unchanged. The `codename(n)` filter produces deterministic friendly names from an input string: `codename(1)` returns a noun, `codename(2)` returns `adjective-noun`, and higher counts add more adjectives. The pool is large (~1.26M combinations for `codename(2)`), so it usually stands alone as a worktree leaf:

```toml
# Friendly branch-derived worktree names, e.g. myproject.malleable-opah
worktree-path = "{{ repo_path }}/../{{ repo }}.{{ branch | codename(2) }}"
```

When you want both a friendly name and the original branch identity in the path, put the branch name in a parent directory:

```toml
worktree-path = "{{ repo_path }}/../worktrees/{{ branch | sanitize }}/{{ branch | codename(2) }}"
```

The `hash` filter is the bare 3-character base36 digest, useful for composing your own truncate-with-collision-avoidance recipes when an output budget is tight (e.g., Unix socket paths capped at 107 bytes):

```toml
# Truncated branch slug + hash: collisions remain disambiguated even when prefixes match
worktree-path = "/tmp/{{ (branch | sanitize)[:20] }}_{{ branch | sanitize | hash }}"
```

The `dirname` and `basename` filters traverse paths. They're useful for bare repos in a hidden directory like `myproject/.git`, where `{{ repo }}` resolves to `.git`:

```toml
# Place worktrees as siblings of the bare repo, named `<wrapper>.<branch>`
worktree-path = "{{ repo_path }}/../{{ repo_path | dirname | basename }}.{{ branch | sanitize }}"
```

The `hash_port` filter is useful for running dev servers on unique ports per worktree:

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
- [Dev server per worktree](@/tips-patterns.md#dev-server-per-worktree): `wt step tether` in `post-start` runs the dev server and kills its whole process group when the worktree is removed, with optional subdomain routing
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

Organizations can deploy a system-wide config file for shared defaults — run `wt config show` for the platform-specific location.

**User config** — personal preferences:

```toml
# ~/.config/worktrunk/config.toml
worktree-path = ".worktrees/{{ branch | sanitize }}"

[commit.generation]
command = "MAX_THINKING_TOKENS=0 claude -p --no-session-persistence --model=haiku --tools='' --safe-mode --setting-sources='user' --system-prompt=''"
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
- `{{ branch | codename(2) }}` — deterministic friendly name from a ~1.26M-combo pool (e.g., `malleable-opah`)

This is a smaller set than [the variables hooks and aliases get](@/hook.md#template-variables).

**Examples** for repo at `~/code/myproject`, branch `feature/auth`:

Default — sibling directory (`~/code/myproject.feature-auth`):

```toml
worktree-path = "{{ repo_path }}/../{{ repo }}.{{ branch | sanitize }}"
```

Inside the repository (`~/code/myproject/.worktrees/feature-auth`):

```toml
worktree-path = "{{ repo_path }}/.worktrees/{{ branch | sanitize }}"
```

Friendly branch-derived names (`~/code/myproject.malleable-opah`):

```toml
worktree-path = "{{ repo_path }}/../{{ repo }}.{{ branch | codename(2) }}"
```

Friendly names with branch identity in a parent directory (`~/code/worktrees/feature-auth/malleable-opah`):

```toml
worktree-path = "{{ repo_path }}/../worktrees/{{ branch | sanitize }}/{{ branch | codename(2) }}"
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
command = "MAX_THINKING_TOKENS=0 claude -p --no-session-persistence --model=haiku --tools='' --safe-mode --setting-sources='user' --system-prompt=''"
```

### Codex

```toml
[commit.generation]
command = "codex exec -m gpt-5.4-mini -c model_reasoning_effort='low' -c system_prompt='' --sandbox=read-only --json - | jq -sr '[.[] | select(.item.type? == \"agent_message\")] | last.item.text'"
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

full = false       # Show CI status and LLM summaries (--full)
branches = false   # Include branches without worktrees (--branches)
remotes = false    # Include remote-only branches (--remotes)

json-schema = 1    # JSON output schema: 1 (current, bare array) or 2 (envelope); unset emits 1 with a warning

columns = ["branch", "status", "ci", "path"]   # Columns to show, in order — built-ins or custom headers (omit for the default set)

task-timeout-ms = 0   # Kill individual git commands after N ms; 0 disables
timeout-ms = 0        # Wall-clock budget for the entire collect phase; 0 disables
```

`columns` selects and orders the columns to render; omit it for the default set.
It is meant to drive a per-invocation [alias](@/extending.md#aliases)
(`wt --config-set 'list.columns=[…]' list`), giving a named view without
disturbing the default `wt list`. A static setting works but pins one layout
over a table that otherwise adapts to `--full` and terminal width.

Valid built-in names:

- `branch` — The branch name
- `status` — Git status symbols, plus any user-defined status
- `working-diff` — Uncommitted line changes against `HEAD` (header `HEAD±`)
- `ahead-behind` — Commits ahead of and behind the default branch (header `main↕`)
- `branch-diff` — Line changes against the default branch (header `main…±`)
- `summary` — An LLM-generated summary of the branch
- `upstream` — Commits ahead of and behind the upstream tracking branch (header `Remote⇅`)
- `ci` — CI status of the head commit
- `path` — The worktree's path
- `url` — Dev-server URL from the `[list] url` template
- `commit` — The head commit's short hash
- `age` — Time since the last commit
- `message` — The head commit's subject

A selection mixes built-ins with [custom columns](#custom-columns), each named
by its `[list.custom-columns]` header (`columns = ["branch", "Ticket", "ci"]`),
and is exhaustive: only the listed columns render. Omit `columns` to keep the
default set, where custom columns append automatically. A built-in name wins a
header collision; the gutter type indicator always shows.

Listing a column forces it on, space permitting: `ci` shows without `--full`,
since `--full` only bundles columns into the default table rather than gating a
named one. A column whose data source is missing still stays hidden — `summary`
needs an LLM command (`[commit.generation]`), `url` needs a `[list] url`
template — since listing can't supply the data.

The selection drives the table and the `wt switch` picker. `wt list --format
json` always emits every field, but a listed gated column (`ci`, `summary`)
still forces its data collection on, so the JSON carries the same data the
table shows.

#### Custom columns [experimental]

Custom columns add per-branch context to the `wt list` table. Each
`[list.custom-columns]` entry is a column: the key is the header, the template
renders each row's cell.

```toml
[list.custom-columns.Ticket]
template = "{{ vars.ticket }}"   # Required; the result is the cell text
width = 20                       # Optional max display width (default: 40)
priority = 9                     # Optional drop order when the terminal narrows;
                                 # lower = kept longer (default: 9, the URL band)
```

Templates may reference `{{ branch }}`, `{{ worktree_path }}`,
`{{ worktree_name }}` (empty for branch-only rows), and two per-branch
namespaces:

- `{{ vars.* }}` — values stored with
  [`wt config state vars set`](@/config.md#wt-config-state-vars).
- `{{ git.branch.* }}` — the branch's own git config under `branch.<name>.*`,
  read straight from `git config` (e.g. `{{ git.branch.jira }}` for a key you
  set yourself, or the git-native `description`). Git lowercases config variable
  names, so `branch.<name>.nvciShelf` reads as `{{ git.branch.nvcishelf }}`.

All standard filters work (`sanitize`, `hash_port`, `codename`, …). A row
where the template renders empty (e.g. a branch without the key) shows an
empty cell; a column that is empty for every row is dropped from the table.
`wt list --format json` includes the rendered values under `columns`.

A `Jira` column reading a key kept in git config, and a `Summary` column
showing just the first line of the git-native branch description:

```toml
[list.custom-columns.Jira]
template = "{{ git.branch.jira }}"

[list.custom-columns.Summary]
template = "{{ git.branch.description | lines | first }}"
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

### Remove

Persistent flag values for `wt remove`. Override on command line as needed.

```toml
[remove]
delete-branch = true   # Delete branch after removal (--no-delete-branch to keep)
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

Built-in excludes always apply: VCS metadata directories (`.bzr/`, `.hg/`, `.jj/`, `.pijul/`, `.sl/`, `.svn/`) and tool-state directories (`.conductor/`, `.entire/`, `.worktrees/`). User config and project config exclusions are combined.

### Aliases

Command templates that run as `wt <name>`. See the [Extending Worktrunk guide](@/extending.md#aliases) for usage and flags.

```toml
[aliases]
greet = "echo Hello from {{ branch }}"
url = "echo http://localhost:{{ branch | hash_port }}"
```

Aliases defined here apply to all projects. For project-specific aliases, use the [project config](@/config.md#project-configuration) `[aliases]` section instead.

### User project-specific settings

User config can include a `[projects]` table for project-specific settings — worktree layout, setting overrides, anything else — separate from the [project config](@/config.md#project-configuration) shared with teammates.

Entries are keyed by project identifier — `<host>/<owner>/<repo>` derived from the primary remote URL (no `.git` suffix), or the canonical repo path when there is no remote. Run `wt config show` inside the repo to see the identifier for the current project; it appears in the `PROJECT CONFIG` section as `Identifier: …`.

Scalar values (like `worktree-path`) replace the global value; everything else (hooks, aliases, etc.) appends, global first.

```toml
[projects."github.com/user/repo"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
list.full = true
merge.squash = false
remove.delete-branch = false
pre-start.env = "cp .env.example .env"
step.copy-ignored.exclude = [".repo-local-cache/"]
aliases.deploy = "make deploy BRANCH={{ branch }}"
```

Hooks support all three [hook forms](@/hook.md#hook-forms). A table runs multiple commands concurrently; an array-of-tables pipeline runs steps in sequence. The dotted-key examples below are equivalent to the table forms — TOML treats `projects."github.com/user/repo".post-start.server = "..."` and a `[projects."github.com/user/repo".post-start]` table the same way:

```toml
# Single command
[projects."github.com/user/repo"]
post-start = "mise trust"

# Multiple commands, running concurrently
[projects."github.com/user/repo".post-start]
mise = "mise trust"
server = "npm run dev"

# Pipeline: steps run in sequence
[[projects."github.com/user/repo".post-start]]
install = "npm ci"

[[projects."github.com/user/repo".post-start]]
build = "npm run build"
server = "npm run dev"
```

### Custom prompt templates

Templates use [minijinja](https://docs.rs/minijinja/) syntax.

#### Commit template

Available variables:

- `{{ git_diff }}`, `{{ git_diff_stat }}` — diff content
- `{{ branch }}`, `{{ repo }}` — context
- `{{ recent_commits }}` — recent commit messages
- `{{ user_guidance }}`, `{{ project_guidance }}` — rendered append fragments (see [Appending to the prompt](@/config.md#appending-to-the-prompt))

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
{% if user_guidance %}
<user-guidance>
{{ user_guidance }}
</user-guidance>
{% endif %}{% if project_guidance %}
<project-guidance>
{{ project_guidance }}
</project-guidance>
{% endif %}
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

- `{{ commit_details }}` — list of commits being squashed; each renders as its subject and exposes `.subject` / `.body`
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
{% if user_guidance %}
<user-guidance>
{{ user_guidance }}
</user-guidance>
{% endif %}{% if project_guidance %}
<project-guidance>
{{ project_guidance }}
</project-guidance>
{% endif %}
<commits branch="{{ branch }}" target="{{ target_branch }}">
{% for detail in commit_details %}- {{ detail.subject }}
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

#### Appending to the prompt [experimental]

`template-append` adds to the prompt instead of replacing it. The value is rendered as its own minijinja template (same variables) and injected into the default templates' `{{ user_guidance }}` slot — a `<user-guidance>` block right after `<style>`. It applies to both commit and squash. Use it for personal preferences without restating the whole template:

```toml
[commit.generation]
template-append = """
- Explain the rationale in the body, not just the change
"""
```

The [project config](@/config.md#project-configuration) has a `template-append` of its own; it renders into a separate `<project-guidance>` block right after `<user-guidance>`.

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

## Forge platform

Name the forge explicitly for SSH aliases or self-hosted instances, where it can't be detected from the remote URL:

```toml
[forge]
platform = "github"  # or "gitlab", "gitea" (experimental), "azure-devops" (experimental)
hostname = "github.example.com"  # Example: API host (GHE / self-hosted GitLab)
```

## Commit-message append [experimental]

Project-wide commit-message conventions appended to the LLM commit and squash prompts inside a `<project-guidance>` block, after the main template's `<style>` section (and after any user `<user-guidance>`). Rendered as a [minijinja](https://docs.rs/minijinja/) template with the same variables as the main commit template (`{{ branch }}`, `{{ git_diff }}`, etc.), so it can reference them directly. The first time the fragment changes, `wt` prompts the user to approve it — the same one-shot gate as project-defined hooks.

```toml
[commit.generation]
template-append = """
- Use conventional commits (feat:, fix:, docs:, …)
- Reference the relevant issue ID in the body
"""
```

Only `template-append` is honored from the project file. The LLM command and the main prompt template stay in [user config](@/config.md) — they describe per-developer environment (which CLI is installed, which agent the developer prefers). User config has a `[commit.generation] template-append` of its own; it renders into a separate `<user-guidance>` block immediately before this one.

## Copy-ignored excludes

Additional excludes for `wt step copy-ignored`:

```toml
[step.copy-ignored]
exclude = [".cache/", ".turbo/"]
```

Built-in excludes always apply: VCS metadata directories (`.bzr/`, `.hg/`, `.jj/`, `.pijul/`, `.sl/`, `.svn/`) and tool-state directories (`.conductor/`, `.entire/`, `.worktrees/`). User config and project config exclusions are combined.

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

On first run without shell integration, Worktrunk offers to install it. On first commit without LLM configuration, it offers to configure a detected tool (`claude`, `codex`). Declining sets `skip-shell-integration-prompt` or `skip-commit-generation-prompt` automatically.

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
| `WORKTRUNK_VERBOSE` | Verbosity level (`0`/`1`/`2`), like `-v`/`-vv` but applied everywhere — including shell completion, which no flag can reach |
| `RUST_LOG` | Logging directive (e.g. `worktrunk=debug`); overrides the verbosity baseline for what reaches stderr |
| `NO_COLOR` | Disable colored output ([standard](https://no-color.org/)) |
| `CLICOLOR_FORCE` | Force colored output even when not a TTY |

## Inline config overrides (`--config-set`)

`--config-set <toml>` overrides any user config key for a single invocation, with higher priority than both config files and `WORKTRUNK_` env vars. The value is a TOML fragment, so arrays and tables work directly; the flag is global (works before or after the subcommand), repeatable, and a later `--config-set` replaces an earlier one for the same key.

```console
$ wt --config-set list.full=true list
$ wt step copy-ignored --config-set 'step.copy-ignored.exclude=["target", "dist"]'
```

This composes with aliases — an alias body can invoke `wt --config-set … <command>` to render a named view without changing the saved config.
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

#[cfg(test)]
mod tests {
    use super::{non_empty_branch, resolve_version};

    #[test]
    fn non_empty_branch_rejects_blank() {
        assert_eq!(non_empty_branch("feature").unwrap(), "feature");
        assert!(non_empty_branch("").is_err());
        assert!(non_empty_branch("   ").is_err());
    }

    #[test]
    fn resolve_version_uses_git_describe_when_available() {
        assert_eq!(
            resolve_version(Some("v0.8.5-3-gabcdef"), "0.8.5"),
            "v0.8.5-3-gabcdef"
        );
    }

    #[test]
    fn resolve_version_falls_back_when_git_describe_idempotent() {
        // vergen emits an IDEMPOTENT placeholder for non-reproducible builds.
        assert_eq!(
            resolve_version(Some("VERGEN_IDEMPOTENT_OUTPUT"), "0.8.5"),
            "0.8.5"
        );
    }

    #[test]
    fn resolve_version_falls_back_when_git_describe_absent() {
        // Building from the crates.io package archive: no git worktree, so the
        // build script never sets VERGEN_GIT_DESCRIBE (#3123).
        assert_eq!(resolve_version(None, "0.59.0"), "0.59.0");
    }
}
