mod config;
mod hook;
mod list;
mod step;

pub use config::{
    ApprovalsCommand, CiStatusAction, ConfigCommand, ConfigShellCommand, DefaultBranchAction,
    HintsAction, LogsAction, MarkerAction, PreviousBranchAction, StateCommand,
};
pub use hook::HookCommand;
pub use list::ListSubcommand;
pub use step::StepCommand;

use clap::builder::styling::{AnsiColor, Color, Styles};
use clap::{Command, CommandFactory, Parser, Subcommand, ValueEnum};
use std::sync::OnceLock;
use worktrunk::config::{DEPRECATED_TEMPLATE_VARS, TEMPLATE_VARS};

use crate::commands::Shell;

/// Parse key=value string into a tuple, validating that the key is a known template variable.
///
/// Used by the `--var` flag on hook commands to override built-in template variables.
/// Values are shell-escaped during template expansion (see `expand_template` in expansion.rs).
pub(super) fn parse_key_val(s: &str) -> Result<(String, String), String> {
    let (key, value) = s
        .split_once('=')
        .ok_or_else(|| format!("invalid KEY=VALUE: no `=` found in `{s}`"))?;
    if key.is_empty() {
        return Err("invalid KEY=VALUE: key cannot be empty".to_string());
    }
    if !TEMPLATE_VARS.contains(&key) && !DEPRECATED_TEMPLATE_VARS.contains(&key) {
        return Err(format!(
            "unknown variable `{key}`; valid variables: {} (deprecated: {})",
            TEMPLATE_VARS.join(", "),
            DEPRECATED_TEMPLATE_VARS.join(", ")
        ));
    }
    Ok((key.to_string(), value.to_string()))
}

/// Custom styles for help output - matches worktrunk's color scheme
fn help_styles() -> Styles {
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
pub fn build_command() -> Command {
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
pub fn suggest_nested_subcommand(cmd: &Command, unknown: &str) -> Option<String> {
    for parent in NESTED_COMMAND_PARENTS {
        if let Some(parent_cmd) = cmd.get_subcommands().find(|c| c.get_name() == *parent)
            && parent_cmd
                .get_subcommands()
                .any(|s| s.get_name() == unknown)
        {
            return Some(format!("wt {parent} {unknown}"));
        }
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
pub fn version_str() -> &'static str {
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

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum OutputFormat {
    /// Human-readable table format
    Table,
    /// JSON output
    Json,
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
pub struct Cli {
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

    /// Show debug info (-v), or also write diagnostic report (-vv)
    #[arg(
        long,
        short = 'v',
        global = true,
        action = clap::ArgAction::Count,
        display_order = 102,
        help_heading = "Global Options"
    )]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Switch to a worktree
    #[command(
        after_long_help = r#"Change directory to a worktree, creating one if needed.
<!-- demo: wt-switch.gif 1600x900 -->

Worktrees are addressed by branch name; paths are computed from a configurable template. Unlike `git switch`, this navigates between worktrees rather than changing branches in place.

## Examples

```console
wt switch feature-auth           # Switch to worktree
wt switch -                      # Previous worktree (like cd -)
wt switch --create new-feature   # Create new branch and worktree
wt switch --create hotfix --base production
```

## Creating a branch

The `--create` flag creates a new branch from the `--base` branch (defaults to default branch). Without `--create`, the branch must already exist.

## Creating worktrees

If the branch already has a worktree, `wt switch` changes directories to it. Otherwise, it creates one, running [hooks](@/hook.md).

When creating a worktree, worktrunk:

1. Creates worktree at configured path
2. Switches to new directory
3. Runs [post-create hooks](@/hook.md#post-create) (blocking)
4. Spawns [post-start hooks](@/hook.md#post-start) (background)

```console
wt switch feature                        # Existing branch → creates worktree
wt switch --create feature               # New branch and worktree
wt switch --create fix --base release    # New branch from release
wt switch --create temp --no-verify      # Skip hooks
```

## Shortcuts

| Shortcut | Meaning |
|----------|---------|
| `^` | Default branch (`main`/`master`) |
| `@` | Current branch/worktree |
| `-` | Previous worktree (like `cd -`) |

```console
wt switch -                      # Back to previous
wt switch ^                      # Default branch worktree
wt switch --create fix --base=@  # Branch from current HEAD
```

## When wt switch fails

- **Branch doesn't exist** — Use `--create`, or check `wt list --branches`
- **Path occupied** — Another worktree is at the target path; switch to it or remove it
- **Stale directory** — Use `--clobber` to remove a non-worktree directory at the target path

To change which branch a worktree is on, use `git switch` inside that worktree.

## See also

- [`wt select`](@/select.md) — Interactive worktree selection
- [`wt list`](@/list.md) — View all worktrees
- [`wt remove`](@/remove.md) — Delete worktrees when done
- [`wt merge`](@/merge.md) — Integrate changes back to the default branch
"#
    )]
    Switch {
        /// Branch name
        ///
        /// Shortcuts: '^' (default branch), '-' (previous), '@' (current)
        #[arg(add = crate::completion::worktree_branch_completer())]
        branch: String,

        /// Create a new branch
        #[arg(short = 'c', long)]
        create: bool,

        /// Base branch
        ///
        /// Defaults to default branch.
        #[arg(short = 'b', long, add = crate::completion::branch_value_completer())]
        base: Option<String>,

        /// Command to run after switch
        ///
        /// Replaces the wt process with the command after switching, giving
        /// it full terminal control. Useful for launching editors, AI agents,
        /// or other interactive tools.
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
        #[arg(short = 'x', long)]
        execute: Option<String>,

        /// Additional arguments for --execute command (after --)
        ///
        /// Arguments after `--` are appended to the execute command.
        /// Each argument is POSIX shell-escaped before appending.
        #[arg(last = true, requires = "execute")]
        execute_args: Vec<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// Remove stale paths at target
        #[arg(long)]
        clobber: bool,

        /// Skip hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,
    },

    /// List worktrees and their status
    #[command(
        after_long_help = r#"Show all worktrees with their status. The table includes uncommitted changes, divergence from the default branch and remote, and optional CI status.
<!-- demo: wt-list.gif 1600x900 -->

The table renders progressively: branch names, paths, and commit hashes appear immediately, then status, divergence, and other columns fill in as background git operations complete. With `--full`, CI status fetches from the network — the table displays instantly and CI fills in as results arrive.

## Examples

List all worktrees:

<!-- wt list -->
```console
$ wt list
```

Include CI status and line diffs:

<!-- wt list --full -->
```console
$ wt list --full
```

Include branches that don't have worktrees:

<!-- wt list --branches --full -->
```console
$ wt list --branches --full
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
| main…± | Line diffs since the merge-base with the default branch (`--full`) |
| Path | Worktree directory |
| Remote⇅ | Commits ahead/behind tracking branch |
| URL | Dev server URL from project config (dimmed if port not listening) |
| CI | Pipeline status (`--full`) |
| Commit | Short hash (8 chars) |
| Age | Time since last commit |
| Message | Last commit message (truncated) |

Note: `main↕` and `main…±` refer to the default branch (header label stays `main` for compactness). `main…±` uses a merge-base (three-dot) diff.

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

CI indicators are clickable links to the PR or pipeline page. Any CI dot appears dimmed when there are unpushed local changes (stale status). PRs/MRs are checked first, then branch workflows/pipelines for branches with an upstream. Local-only branches show blank. Results are cached for 30-60 seconds; use `wt config state` to view or clear.

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
| | `✗` | Would conflict if merged to the default branch (with `--full`, includes uncommitted changes) |
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

## JSON output

Query structured data with `--format=json`:

```console
# Current worktree path (for scripts)
wt list --format=json | jq -r '.[] | select(.is_current) | .path'

# Branches with uncommitted changes
wt list --format=json | jq '.[] | select(.working_tree.modified)'

# Worktrees with merge conflicts
wt list --format=json | jq '.[] | select(.operation_state == "conflicts")'

# Branches ahead of main (needs merging)
wt list --format=json | jq '.[] | select(.main.ahead > 0) | .branch'

# Integrated branches (safe to remove)
wt list --format=json | jq '.[] | select(.main_state == "integrated" or .main_state == "empty") | .branch'

# Branches without worktrees
wt list --format=json --branches | jq '.[] | select(.kind == "branch") | .branch'

# Worktrees ahead of remote (needs pushing)
wt list --format=json | jq '.[] | select(.remote.ahead > 0) | {branch, ahead: .remote.ahead}'

# Stale CI (local changes not reflected in CI)
wt list --format=json --full | jq '.[] | select(.ci.stale) | .branch'
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
| `operation_state` | string | `"conflicts"`, `"rebase"`, or `"merge"` (absent when clean) |
| `main` | object | Relationship to the default branch (see below, absent when is_main) |
| `remote` | object | Tracking branch info (see below, absent when no tracking) |
| `worktree` | object | Worktree metadata (see below) |
| `is_main` | boolean | Is the main worktree |
| `is_current` | boolean | Is the current worktree |
| `is_previous` | boolean | Previous worktree from wt switch |
| `ci` | object | CI status (see below, absent when no CI) |
| `url` | string | Dev server URL from project config (absent when not configured) |
| `url_active` | boolean | Whether the URL's port is listening (absent when not configured) |
| `statusline` | string | Pre-formatted status with ANSI colors |
| `symbols` | string | Raw status symbols without colors (e.g., `"!?↓"`) |

### commit object

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
| `diff_vs_main` | object | Lines changed vs the default branch: `{added, deleted}` |

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
| `state` | string | `"branch_worktree_mismatch"`, `"prunable"`, `"locked"` (absent when normal) |
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

These values describe relation to the default branch.

`"is_main"` `"would_conflict"` `"empty"` `"same_commit"` `"integrated"` `"diverged"` `"ahead"` `"behind"`

### integration_reason values

When `main_state == "integrated"`: `"ancestor"` `"trees_match"` `"no_added_changes"` `"merge_adds_nothing"`

### ci.status values

`"passed"` `"running"` `"failed"` `"conflicts"` `"no-ci"` `"error"`

Missing a field that would be generally useful? Open an issue at https://github.com/max-sixty/worktrunk.

## See also

- [`wt select`](@/select.md) — Interactive worktree picker with live preview
"#
    )]
    // TODO: `args_conflicts_with_subcommands` causes confusing errors for unknown
    // subcommands ("cannot be used with --branches") instead of "unknown subcommand".
    // Could fix with external_subcommand + post-parse validation, but not worth the
    // code. The `statusline` subcommand may move elsewhere anyway.
    #[command(args_conflicts_with_subcommands = true)]
    List {
        #[command(subcommand)]
        subcommand: Option<ListSubcommand>,

        /// Output format (table, json)
        #[arg(long, value_enum, default_value = "table", hide_possible_values = true)]
        format: OutputFormat,

        /// Include branches without worktrees
        #[arg(long)]
        branches: bool,

        /// Include remote branches
        #[arg(long)]
        remotes: bool,

        /// Include CI status and diff analysis (slower)
        #[arg(long)]
        full: bool,

        /// Show fast info immediately, update with slow info
        ///
        /// Displays local data (branches, paths, status) first, then updates
        /// with remote data (CI, upstream) as it arrives. Auto-enabled for TTY.
        #[arg(long, overrides_with = "no_progressive")]
        progressive: bool,

        /// Force buffered rendering
        #[arg(long = "no-progressive", overrides_with = "progressive", hide = true)]
        no_progressive: bool,
    },

    /// Remove worktree; delete branch if merged
    #[command(
        after_long_help = r#"Removes worktrees and their branches (if merged), returning to the main worktree. Defaults to removing the current worktree.

## Examples

Remove current worktree:

```console
wt remove
```

Remove specific worktrees:

```console
wt remove feature-branch
wt remove old-feature another-branch
```

Keep the branch:

```console
wt remove --no-delete-branch feature-branch
```

Force-delete an unmerged branch:

```console
wt remove -D experimental
```

## Branch cleanup

By default, branches are deleted when merging them would add nothing. This works with squash-merge and rebase workflows where commit history differs but file changes match.

Worktrunk checks five conditions (in order of cost):

1. **Same commit** — Branch HEAD equals the default branch. Shows `_` in `wt list`.
2. **Ancestor** — Branch is in target's history (fast-forward or rebase case). Shows `⊂`.
3. **No added changes** — Three-dot diff (`target...branch`) is empty. Shows `⊂`.
4. **Trees match** — Branch tree SHA equals target tree SHA. Shows `⊂`.
5. **Merge adds nothing** — Simulated merge produces the same tree as target. Handles squash-merged branches where target has advanced. Shows `⊂`.

The "same commit" check uses the local default branch; for other checks, **target** means the default branch, or its upstream (e.g., `origin/main`) when strictly ahead.

Branches showing `_` or `⊂` are dimmed as safe to delete.

Use `-D` to force-delete branches with unmerged changes. Use `--no-delete-branch` to keep the branch regardless of status.

## Background removal

Removal runs in the background by default (returns immediately). Logs are written to `.git/wt-logs/{branch}-remove.log`. Use `--foreground` to run in the foreground.

## Shortcuts

`@` (current), `-` (previous), `^` (default branch). See [`wt switch`](@/switch.md#shortcuts).

## See also

- [`wt merge`](@/merge.md) — Remove worktree after merging
- [`wt list`](@/list.md) — View all worktrees
"#
    )]
    Remove {
        /// Branch name [default: current]
        #[arg(add = crate::completion::local_branches_completer())]
        branches: Vec<String>,

        /// Keep branch after removal
        #[arg(long = "no-delete-branch", action = clap::ArgAction::SetFalse, default_value_t = true)]
        delete_branch: bool,

        /// Delete unmerged branches
        #[arg(short = 'D', long = "force-delete")]
        force_delete: bool,

        /// Run removal in foreground (block until complete)
        #[arg(long)]
        foreground: bool,

        /// Deprecated: use --foreground instead
        #[arg(long = "no-background", hide = true)]
        no_background: bool,

        /// Skip hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,

        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// Force worktree removal
        ///
        /// Remove worktrees even if they contain untracked files (like build
        /// artifacts). Without this flag, removal fails if untracked files exist.
        #[arg(short, long)]
        force: bool,
    },

    /// Merge worktree into target branch
    ///
    /// Squash & rebase, fast-forward target, remove the worktree.
    #[command(
        after_long_help = r#"Merge the current branch into the default branch — like clicking "Merge pull request" on GitHub.
<!-- demo: wt-merge.gif 1600x900 -->

## Examples

Merge to the default branch:

```console
wt merge
```

Merge to a different branch:

```console
wt merge develop
```

Keep the worktree after merging:

```console
wt merge --no-remove
```

Preserve commit history (no squash):

```console
wt merge --no-squash
```

Skip committing/squashing (rebase still runs unless --no-rebase):

```console
wt merge --no-commit
```

## Pipeline

`wt merge` runs these steps:

1. **Squash** — Stages uncommitted changes, then combines all commits since target into one (like GitHub's "Squash and merge"). Use `--stage` to control what gets staged: `all` (default), `tracked`, or `none`. A backup ref is saved to `refs/wt-backup/<branch>`. With `--no-squash`, uncommitted changes become a separate commit and individual commits are preserved.
2. **Rebase** — Rebases onto target if behind. Skipped if already up-to-date. Conflicts abort immediately.
3. **Pre-merge hooks** — Hooks run after rebase, before merge. Failures abort. See [`wt hook`](@/hook.md).
4. **Merge** — Fast-forward merge to the target branch. Non-fast-forward merges are rejected.
5. **Pre-remove hooks** — Hooks run before removing worktree. Failures abort.
6. **Cleanup** — Removes the worktree and branch. Use `--no-remove` to keep the worktree. When already on the target branch or in the main worktree, the worktree is preserved.
7. **Post-merge hooks** — Hooks run after cleanup. Failures are logged but don't abort.

Use `--no-commit` to skip committing uncommitted changes and squashing; rebase still runs by default and can rewrite commits unless `--no-rebase` is passed. Useful after preparing commits manually with `wt step`. Requires a clean working tree.

## Local CI

For personal projects, pre-merge hooks open up the possibility of a workflow with much faster iteration — an order of magnitude more small changes instead of fewer large ones.

Historically, ensuring tests ran before merging was difficult to enforce locally. Remote CI was valuable for the process as much as the checks: it guaranteed validation happened. `wt merge` brings that guarantee local.

The full workflow: start an agent (one of many) on a task, work elsewhere, return when it's ready. Review the diff, run `wt merge`, move on. Pre-merge hooks validate before merging — if they pass, the branch goes to the default branch and the worktree cleans up.

```toml
[pre-merge]
test = "cargo test"
lint = "cargo clippy"
```

## See also

- [`wt step`](@/step.md) — Run individual operations (commit, squash, rebase, push)
- [`wt remove`](@/remove.md) — Remove worktrees without merging
- [`wt switch`](@/switch.md) — Navigate to other worktrees
"#
    )]
    Merge {
        /// Target branch
        ///
        /// Defaults to default branch.
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,

        /// Force commit squashing
        #[arg(long, overrides_with = "no_squash", hide = true)]
        squash: bool,

        /// Skip commit squashing
        #[arg(long = "no-squash", overrides_with = "squash")]
        no_squash: bool,

        /// Force commit and squash
        #[arg(long, overrides_with = "no_commit", hide = true)]
        commit: bool,

        /// Skip commit and squash
        #[arg(long = "no-commit", overrides_with = "commit")]
        no_commit: bool,

        /// Force rebasing onto target
        #[arg(long, overrides_with = "no_rebase", hide = true)]
        rebase: bool,

        /// Skip rebase (fail if not already rebased)
        #[arg(long = "no-rebase", overrides_with = "rebase")]
        no_rebase: bool,

        /// Force worktree removal after merge
        #[arg(long, overrides_with = "no_remove", hide = true)]
        remove: bool,

        /// Keep worktree after merge
        #[arg(long = "no-remove", overrides_with = "remove")]
        no_remove: bool,

        /// Force running hooks
        #[arg(long, overrides_with = "no_verify", hide = true)]
        verify: bool,

        /// Skip hooks
        #[arg(long = "no-verify", overrides_with = "verify")]
        no_verify: bool,

        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// What to stage before committing [default: all]
        #[arg(long)]
        stage: Option<crate::commands::commit::StageMode>,
    },
    /// Interactive worktree selector
    ///
    /// Browse and switch worktrees with live preview.
    #[cfg_attr(not(unix), command(hide = true))]
    #[command(
        after_long_help = r#"Interactive worktree picker with live preview. Navigate worktrees with keyboard shortcuts and press Enter to switch.
<!-- demo: wt-select.gif 1600x800 -->

## Examples

Open the selector:

```console
wt select
```

## Preview tabs

Toggle between views with number keys:

1. **HEAD±** — Diff of uncommitted changes
2. **log** — Recent commits; commits already on the default branch have dimmed hashes
3. **main…±** — Diff of changes since the merge-base with the default branch
4. **remote⇅** — Diff vs upstream tracking branch (ahead/behind)

## Keybindings

| Key | Action |
|-----|--------|
| `↑`/`↓` | Navigate worktree list |
| `Enter` | Switch to selected worktree |
| `Esc` | Cancel |
| (type) | Filter worktrees |
| `1`/`2`/`3`/`4` | Switch preview tab |
| `Alt-p` | Toggle preview panel |
| `Ctrl-u`/`Ctrl-d` | Scroll preview up/down |

With `--branches`, branches without worktrees are included — selecting one creates a worktree. This matches `wt list --branches`.

## Configuration

### Pager

The preview panel pipes diff output through git's pager (typically `less` or `delta`). Override pager behavior in user config:

```toml
[select]
pager = "delta --paging=never"
```

This is useful when the default pager doesn't render correctly in the embedded preview panel.

## See also

- [`wt list`](@/list.md) — Static table view with all worktree metadata
- [`wt switch`](@/switch.md) — Direct switching to a known target branch
"#
    )]
    Select {
        /// Include branches without worktrees
        #[arg(long)]
        branches: bool,

        /// Include remote branches
        #[arg(long)]
        remotes: bool,
    },

    /// Run individual operations
    #[command(
        name = "step",
        after_long_help = r#"Run individual git workflow operations: commits, squashes, rebases, and pushes.

## Examples

Commit with LLM-generated message:

```console
wt step commit
```

Manual merge workflow with review between steps:

```console
wt step commit
wt step squash
# Review the squashed commit
wt step rebase
wt step push
```

## Operations

- `commit` — Stage and commit with [LLM-generated message](@/llm-commits.md)
- `squash` — Squash all branch commits into one with [LLM-generated message](@/llm-commits.md)
- `rebase` — Rebase onto target branch
- `push` — Fast-forward target to current branch
- `copy-ignored` — Copy gitignored files between worktrees
- `for-each` — [experimental] Run a command in every worktree

## Options

### `--stage`

Controls what to stage before committing. Available for `commit` and `squash`:

| Value | Behavior |
|-------|----------|
| `all` | Stage all changes including untracked files (default) |
| `tracked` | Stage only modified tracked files |
| `none` | Don't stage anything, commit only what's already staged |

```bash
wt step commit --stage=tracked
wt step squash --stage=none
```

Configure the default in user config:

```toml
[commit]
stage = "tracked"
```

### `--show-prompt`

Output the rendered LLM prompt to stdout without running the command. Useful for inspecting prompt templates or piping to other tools:

```bash
# Inspect the rendered prompt
wt step commit --show-prompt | less

# Pipe to a different LLM
wt step commit --show-prompt | llm -m gpt-5-nano
```

## See also

- [`wt merge`](@/merge.md) — Runs commit → squash → rebase → hooks → push → cleanup automatically
- [`wt hook`](@/hook.md) — Run configured hooks

<!-- subdoc: copy-ignored -->

<!-- subdoc: for-each -->
"#
    )]
    Step {
        #[command(subcommand)]
        action: StepCommand,
    },

    /// Run configured hooks
    #[command(
        name = "hook",
        after_long_help = r#"Shell commands that run at key points in the worktree lifecycle.

Hooks run automatically during `wt switch`, `wt merge`, & `wt remove`. `wt hook <type>` runs them on demand. Both user hooks (from `~/.config/worktrunk/config.toml`) and project hooks (from `.config/wt.toml`) are supported.

## Hook types

| Hook | When | Blocking | Fail-fast |
|------|------|----------|-----------|
| `post-create` | After worktree created | Yes | No |
| `post-start` | After worktree created | No (background) | No |
| `post-switch` | After every switch | No (background) | No |
| `pre-commit` | Before commit during merge | Yes | Yes |
| `pre-merge` | Before merging to target | Yes | Yes |
| `post-merge` | After successful merge | Yes | No |
| `pre-remove` | Before worktree removed | Yes | Yes |

**Blocking**: Command waits for hook to complete before continuing.
**Fail-fast**: First failure aborts the operation.

### post-create

Runs after worktree creation, **blocks until complete**. The worktree switch doesn't finish until these commands succeed.

**Use cases**: Installing dependencies, database migrations, copying environment files.

```toml
[post-create]
install = "npm ci"
migrate = "npm run db:migrate"
env = "cp .env.example .env"
```

### post-start

Runs after worktree creation, **in background**. The worktree switch completes immediately; these run in parallel.

**Use cases**: Long builds, dev servers, file watchers, downloading large assets.

```toml
[post-start]
build = "npm run build"
server = "npm run dev"
```

Output logged to `.git/wt-logs/{branch}-{source}-post-start-{name}.log` (source is `user` or `project`).

### post-switch

Runs after **every** switch operation, **in background**. Triggers on all switch results: creating new worktrees, switching to existing ones, or switching to the current worktree.

**Use cases**: Renaming terminal tabs, updating tmux window names, IDE notifications.

```toml
post-switch = "echo 'Switched to {{ branch }}'"
```

Output logged to `.git/wt-logs/{branch}-{source}-post-switch-{name}.log` (source is `user` or `project`).

### pre-commit

Runs before committing during `wt merge`, **fail-fast**. All commands must exit 0 for the commit to proceed.

**Use cases**: Formatters, linters, type checking.

```toml
[pre-commit]
format = "cargo fmt -- --check"
lint = "cargo clippy -- -D warnings"
```

### pre-merge

Runs before merging to target branch, **fail-fast**. All commands must exit 0 for the merge to proceed.

**Use cases**: Tests, security scans, build verification.

```toml
[pre-merge]
test = "cargo test"
build = "cargo build --release"
```

### post-merge

Runs after successful merge in the **worktree for the target branch** if it exists, otherwise the **main worktree**, **best-effort**. Failures are logged but don't abort.

**Use cases**: Deployment, notifications, installing updated binaries.

```toml
post-merge = "cargo install --path ."
```

### pre-remove

Runs before worktree removal during `wt remove`, **fail-fast**. All commands must exit 0 for removal to proceed.

**Use cases**: Cleanup tasks, saving state, notifying external systems.

```toml
[pre-remove]
cleanup = "rm -rf /tmp/cache/{{ branch }}"
```

### Timing during merge

- **pre-commit** — After staging, before squash commit
- **pre-merge** — After rebase, before merge to target
- **pre-remove** — Before removing worktree during cleanup
- **post-merge** — After cleanup completes

See [`wt merge`](@/merge.md#pipeline) for the complete pipeline.

## Configuration

Hooks are defined in `.config/wt.toml`. They can be a single command or multiple named commands:

```toml
# Single command (string)
post-create = "npm install"

# Multiple commands (table) — run sequentially in declaration order
[pre-merge]
test = "cargo test"
build = "cargo build --release"
```

### Template variables

Hooks can use template variables that expand at runtime:

| Variable | Example | Description |
|----------|---------|-------------|
| `{{ repo }}` | myproject | Repository directory name |
| `{{ repo_path }}` | /path/to/myproject | Absolute path to repository root |
| `{{ branch }}` | feature/auth | Branch name |
| `{{ worktree_name }}` | myproject.feature-auth | Worktree directory name |
| `{{ worktree_path }}` | /path/to/myproject.feature-auth | Absolute worktree path |
| `{{ main_worktree_path }}` | /path/to/myproject | Default branch worktree |
| `{{ default_branch }}` | main | Default branch name |
| `{{ commit }}` | a1b2c3d4e5f6... | Full HEAD commit SHA |
| `{{ short_commit }}` | a1b2c3d | Short HEAD commit SHA |
| `{{ remote }}` | origin | Primary remote name |
| `{{ remote_url }}` | git@github.com:user/repo.git | Remote URL |
| `{{ upstream }}` | origin/feature | Upstream tracking branch |
| `{{ target }}` | main | Target branch (merge hooks only) |
| `{{ base }}` | main | Base branch (creation hooks only) |
| `{{ base_worktree_path }}` | /path/to/myproject | Base branch worktree (creation hooks only) |

See [Designing effective hooks](#designing-effective-hooks) for `main_worktree_path` patterns.

### Filters

Templates support Jinja2 filters for transforming values:

| Filter | Example | Description |
|--------|---------|-------------|
| `sanitize` | `{{ branch \| sanitize }}` | Replace `/` and `\` with `-` |
| `sanitize_db` | `{{ branch \| sanitize_db }}` | Database-safe identifier with hash suffix (`[a-z0-9_]`, max 63 chars) |
| `hash_port` | `{{ branch \| hash_port }}` | Hash to port 10000-19999 |

The `sanitize` filter makes branch names safe for filesystem paths. The `sanitize_db` filter produces database-safe identifiers (lowercase alphanumeric and underscores, no leading digits, with a 3-character hash suffix to avoid collisions and reserved words). The `hash_port` filter is useful for running dev servers on unique ports per worktree:

```toml
[post-start]
dev = "npm run dev -- --host {{ branch }}.lvh.me --port {{ branch | hash_port }}"
```

Hash any string, including concatenations:

```toml
# Unique port per repo+branch combination
dev = "npm run dev --port {{ (repo ~ '-' ~ branch) | hash_port }}"
```

### JSON context

Hooks also receive context as JSON on stdin, enabling hooks in any language:

```python
import json, sys
ctx = json.load(sys.stdin)
print(f"Setting up {ctx['repo']} on branch {ctx['branch']}")
```

The JSON includes all template variables plus `hook_type` and `hook_name`.

## Designing effective hooks

### post-create vs post-start

Both run when creating a worktree. The difference:

| Hook | Execution | Best for |
|------|-----------|----------|
| `post-create` | Blocks until complete | Tasks the developer needs before working (dependency install) |
| `post-start` | Background, parallel | Long-running tasks that don't block worktree creation |

Many tasks work well in `post-start` — they'll likely be ready by the time they're needed, especially when the fallback is recompiling. If unsure, prefer `post-start` for faster worktree creation.

### Copying untracked files

Git worktrees share the repository but not untracked files (dependencies, caches, `.env`). Use [`wt step copy-ignored`](@/step.md#wt-step-copy-ignored) to copy gitignored files:

```toml
[post-create]
copy = "wt step copy-ignored"
```

See [`wt step copy-ignored`](@/step.md#wt-step-copy-ignored) for limiting what gets copied, common patterns, and language-specific notes.

### Dev servers

Run a dev server per worktree on a deterministic port using `hash_port`:

```toml
[post-start]
server = "npm run dev -- --port {{ branch | hash_port }}"
```

The port is stable across machines and restarts — `feature-api` always gets the same port. Show it in `wt list`:

```toml
[list]
url = "http://localhost:{{ branch | hash_port }}"
```

For subdomain-based routing (useful for cookies/CORS), use `lvh.me` which resolves to 127.0.0.1:

```toml
[post-start]
server = "npm run dev -- --host {{ branch | sanitize }}.lvh.me --port {{ branch | hash_port }}"
```

### Databases

Each worktree can have its own database. Docker containers get unique names and ports:

```toml
[post-start]
db = """
docker run -d --rm \
  --name {{ repo }}-{{ branch | sanitize }}-postgres \
  -p {{ ('db-' ~ branch) | hash_port }}:5432 \
  -e POSTGRES_DB={{ branch | sanitize_db }} \
  -e POSTGRES_PASSWORD=dev \
  postgres:16
"""

[pre-remove]
db-stop = "docker stop {{ repo }}-{{ branch | sanitize }}-postgres 2>/dev/null || true"
```

The `('db-' ~ branch)` concatenation hashes differently than plain `branch`, so database and dev server ports don't collide.
Jinja2's operator precedence has pipe `|` with higher precedence than concatenation `~`, meaning expressions need parentheses to filter concatenated values.

Generate `.env.local` with the connection string:

```toml
[post-create]
env = """
cat > .env.local << EOF
DATABASE_URL=postgres://postgres:dev@localhost:{{ ('db-' ~ branch) | hash_port }}/{{ branch | sanitize_db }}
DEV_PORT={{ branch | hash_port }}
EOF
"""
```

## Security

Project commands require approval on first run:

```
▲ repo needs approval to execute 3 commands:

○ post-create install:
   echo 'Installing dependencies...'

❯ Allow and remember? [y/N]
```

- Approvals are saved to user config (`~/.config/worktrunk/config.toml`)
- If a command changes, new approval is required
- Use `--yes` to bypass prompts (useful for CI/automation)
- Use `--no-verify` to skip hooks

Manage approvals with `wt hook approvals add` and `wt hook approvals clear`.

## User hooks

Define hooks in `~/.config/worktrunk/config.toml` to run for all repositories. User hooks run before project hooks and don't require approval.

```toml
# ~/.config/worktrunk/config.toml
[post-create]
setup = "echo 'Setting up worktree...'"

[pre-merge]
notify = "notify-send 'Merging {{ branch }}'"
```

User hooks support the same hook types and template variables as project hooks.

**Key differences from project hooks:**

| Aspect | Project hooks | User hooks |
|--------|--------------|------------|
| Location | `.config/wt.toml` | `~/.config/worktrunk/config.toml` |
| Scope | Single repository | All repositories |
| Approval | Required | Not required |
| Execution order | After user hooks | Before project hooks |

Skip hooks with `--no-verify`. To run a specific hook when user and project both define the same name, use `user:name` or `project:name` syntax.

**Use cases:**
- Personal notifications or logging
- Editor/IDE integration
- Repository-agnostic setup tasks
- Filtering by repository using JSON context

**Filtering by repository:**

User hooks receive JSON context on stdin, enabling repository-specific behavior:

```toml
# ~/.config/worktrunk/config.toml
[post-create]
gitlab-setup = """
python3 -c '
import json, sys, subprocess
ctx = json.load(sys.stdin)
if "gitlab" in ctx.get("remote", ""):
    subprocess.run(["glab", "mr", "create", "--fill"])
'
"""
```

## Running hooks manually

`wt hook <type>` runs hooks on demand — useful for testing during development, running in CI pipelines, or re-running after a failure.

```console
wt hook pre-merge              # Run all pre-merge hooks
wt hook pre-merge test         # Run hooks named "test" from both sources
wt hook pre-merge user:        # Run all user hooks
wt hook pre-merge project:     # Run all project hooks
wt hook pre-merge user:test    # Run only user's "test" hook
wt hook pre-merge project:test # Run only project's "test" hook
wt hook pre-merge --yes        # Skip approval prompts (for CI)
wt hook post-create --var branch=feature/test  # Override template variable
```

The `user:` and `project:` prefixes filter by source. Use `user:` or `project:` alone to run all hooks from that source, or `user:name` / `project:name` to run a specific hook.

The `--var KEY=VALUE` flag overrides built-in template variables — useful for testing hooks with different contexts without switching to that context.

## Language-specific tips

Each ecosystem has quirks that affect hook design. For copying dependencies and caches between worktrees, see [`wt step copy-ignored`](@/step.md#language-specific-notes). This section covers hooks.

### Python

Use `uv sync` to recreate virtual environments:

```toml
[post-create]
install = "uv sync"
```

For pip-based projects without uv:

```toml
[post-create]
venv = "python -m venv .venv && .venv/bin/pip install -r requirements.txt"
```

### Hook flow patterns

**Progressive validation** — Quick checks before commit, thorough validation before merge:

```toml
[pre-commit]
lint = "npm run lint"
typecheck = "npm run typecheck"

[pre-merge]
test = "npm test"
build = "npm run build"
```

**Target-specific behavior** — Different actions for production vs staging:

```toml
post-merge = """
if [ "{{ target }}" = "main" ]; then
    npm run deploy:production
elif [ "{{ target }}" = "staging" ]; then
    npm run deploy:staging
fi
"""
```

## See also

- [`wt merge`](@/merge.md) — Runs hooks automatically during merge
- [`wt switch`](@/switch.md) — Runs post-create/post-start hooks on `--create`
- [`wt config`](@/config.md) — Manage hook approvals

<!-- subdoc: approvals -->
"#
    )]
    Hook {
        #[command(subcommand)]
        action: HookCommand,
    },

    /// Manage configuration and shell integration
    #[command(
        about = "Manage configuration and shell integration",
        after_long_help = r#"Manages configuration, shell integration, and runtime settings.

Worktrunk uses two configuration files:

| File | Location | Purpose |
|------|----------|---------|
| **User config** | `~/.config/worktrunk/config.toml` | Personal settings, command defaults, approved project commands |
| **Project config** | `.config/wt.toml` | Lifecycle hooks, checked into version control |

## Examples

Install shell integration (required for directory switching):

```console
wt config shell install
```

Create user config file with documented examples:

```console
wt config create
```

Create project config file (`.config/wt.toml`) for hooks:

```console
wt config create --project
```

Show current configuration and file locations:

```console
wt config show
```

## User config

The user config stores personal preferences that apply across all repositories. Create it with `wt config create` and view with `wt config show`.

### Worktree path template

Controls where new worktrees are created. The template is relative to the repository root.

**Available variables:**
- `{{ repo }}` — repository directory name
- `{{ branch }}` — raw branch name (e.g., `feature/auth`)
- `{{ branch | sanitize }}` — branch name with `/` and `\` replaced by `-`

**Examples** for a repo at `~/code/myproject` creating branch `feature/login`:

```toml
# Default — siblings in parent directory
# Creates: ~/code/myproject.feature-login
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

# Inside the repository
# Creates: ~/code/myproject/.worktrees/feature-login
worktree-path = ".worktrees/{{ branch | sanitize }}"

# Namespaced (useful when multiple repos share a parent directory)
# Creates: ~/code/worktrees/myproject/feature-login
worktree-path = "../worktrees/{{ repo }}/{{ branch | sanitize }}"

# Nested bare repo (git clone --bare <url> project/.git)
# Creates: ~/code/project/feature-login (sibling to .git)
worktree-path = "../{{ branch | sanitize }}"
```

### Command settings

Set persistent flag values for commands. These apply unless explicitly overridden on the command line.

**`wt list`:**

```toml
[list]
# All off by default
full = true      # --full
branches = true  # --branches
remotes = true   # --remotes
```

**`wt step commit` and `wt merge` staging:**

```toml
[commit]
stage = "all"    # "all" (default), "tracked", or "none"
```

**`wt merge`:**

```toml
[merge]
# These flags are on by default; set to false to disable
squash = false  # Preserve individual commits (--no-squash)
commit = false  # Skip committing uncommitted changes (also disables squash)
rebase = false  # Skip rebase (fails if not already rebased)
remove = false  # Keep worktree after merge (--no-remove)
verify = false  # Skip hooks (--no-verify)
```

### LLM commit messages

Configure automatic commit message generation. Requires an external tool like [llm](https://llm.datasette.io/):

```toml
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]
```

See [LLM Commit Messages](@/llm-commits.md) for setup details and template customization.

### Approved commands

When project hooks run for the first time, Worktrunk prompts for approval. Approved commands are saved here automatically:

```toml
[projects."my-project"]
approved-commands = ["npm ci", "npm test"]
```

Manage approvals with `wt hook approvals add` to review and pre-approve commands, and `wt hook approvals clear` to reset (add `--global` to clear all projects).

### User hooks

Personal hooks that run for all repositories. Use the same syntax as project hooks:

```toml
[post-create]
setup = "echo 'Setting up worktree...'"

[pre-merge]
notify = "notify-send 'Merging {{ branch }}'"
```

User hooks run before project hooks and don't require approval. Skip with `--no-verify`.

See [`wt hook`](@/hook.md#user-hooks) for complete documentation.

## Project config

The project config defines lifecycle hooks and project-specific settings. This file is checked into version control and shared across the team.

Create `.config/wt.toml` in the repository root:

```toml
[post-create]
install = "npm ci"

[pre-merge]
test = "npm test"
lint = "npm run lint"
```

See [`wt hook`](@/hook.md) for complete documentation on hook types, execution order, template variables, and [JSON context](@/hook.md#json-context).

### Dev server URL

The `[list]` section adds a URL column to `wt list`:

```toml
[list]
url = "http://localhost:{{ branch | hash_port }}"
```

URLs are dimmed when the port isn't listening.

### CI platform override

The `[ci]` section overrides CI platform detection for GitHub Enterprise or self-hosted GitLab with custom domains:

```toml
[ci]
platform = "github"  # or "gitlab"
```

By default, the platform is detected from the remote URL. Use this when URL detection fails (e.g., `git.mycompany.com` instead of `github.mycompany.com`).

## Shell integration

Worktrunk needs shell integration to change directories when switching worktrees. Install with:

```console
wt config shell install
```

Or manually add to the shell config:

```console
# For bash: add to ~/.bashrc
eval "$(wt config shell init bash)"

# For zsh: add to ~/.zshrc
eval "$(wt config shell init zsh)"

# For fish: add to ~/.config/fish/config.fish
wt config shell init fish | source
```

Without shell integration, `wt switch` prints the target directory but cannot `cd` into it.

### Skip first-run prompt

On first run without shell integration, Worktrunk offers to install it. Suppress this prompt in CI or automated environments:

```toml
skip-shell-integration-prompt = true
```

Or via environment variable:

```bash
export WORKTRUNK_SKIP_SHELL_INTEGRATION_PROMPT=true
```

## Environment variables

All user config options can be overridden with environment variables using the `WORKTRUNK_` prefix.

### Naming convention

Config keys use kebab-case (`worktree-path`), while env vars use SCREAMING_SNAKE_CASE (`WORKTRUNK_WORKTREE_PATH`). The conversion happens automatically.

For nested config sections, use double underscores to separate levels:

| Config | Environment Variable |
|--------|---------------------|
| `worktree-path` | `WORKTRUNK_WORKTREE_PATH` |
| `commit-generation.command` | `WORKTRUNK_COMMIT_GENERATION__COMMAND` |
| `commit-generation.args` | `WORKTRUNK_COMMIT_GENERATION__ARGS` |

Note the single underscore after `WORKTRUNK` and double underscores between nested keys.

### Array values

Array config values like `args = ["-m", "claude-haiku"]` can be specified as a single string in environment variables:

```console
export WORKTRUNK_COMMIT_GENERATION__ARGS="-m claude-haiku"
```

### Example: CI/testing override

Override the LLM command in CI to use a mock:

```console
WORKTRUNK_COMMIT_GENERATION__COMMAND=echo \
WORKTRUNK_COMMIT_GENERATION__ARGS="test: automated commit" \
  wt merge
```

### Other environment variables

| Variable | Purpose |
|----------|---------|
| `WORKTRUNK_BIN` | Override binary path for shell wrappers (useful for testing dev builds) |
| `WORKTRUNK_CONFIG_PATH` | Override user config file location |
| `WORKTRUNK_DIRECTIVE_FILE` | Internal: set by shell wrappers to enable directory changes |
| `WORKTRUNK_SHELL` | Internal: set by shell wrappers to indicate shell type (e.g., `powershell`) |
| `WORKTRUNK_MAX_CONCURRENT_COMMANDS` | Max parallel git commands (default: 32). Lower if hitting file descriptor limits. |
| `NO_COLOR` | Disable colored output ([standard](https://no-color.org/)) |
| `CLICOLOR_FORCE` | Force colored output even when not a TTY |

<!-- subdoc: create -->

<!-- subdoc: show -->

<!-- subdoc: state -->
"#
    )]
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },
}
