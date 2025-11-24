use clap::builder::styling::{AnsiColor, Color, Styles};
use clap::{Command, CommandFactory, Parser, Subcommand};
use std::sync::OnceLock;

use crate::commands::Shell;

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
{before-help}{name} - {about-with-newline}\
Usage: {usage}

{all-args}{after-help}";

/// Build a clap Command for Cli with the shared help template applied recursively.
pub fn build_command() -> Command {
    let cmd = Cli::command();
    apply_help_template_recursive(cmd, DEFAULT_COMMAND_NAME)
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

fn version_str() -> &'static str {
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
#[command(about = "Git worktree management", long_about = None)]
#[command(version = version_str())]
#[command(disable_help_subcommand = true)]
#[command(styles = help_styles())]
#[command(
    after_long_help = r#"See `wt config --help` for configuration file locations and setup."#
)]
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

    /// Show commands and debug info
    #[arg(
        long,
        short = 'v',
        global = true,
        display_order = 102,
        help_heading = "Global Options"
    )]
    pub verbose: bool,

    /// Use internal mode (outputs directives for shell wrapper)
    #[arg(long, global = true, hide = true)]
    pub internal: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum ConfigShellCommand {
    /// Generate shell integration code
    #[command(after_long_help = r#"## Manual Setup

Add one line to your shell config:

Bash (~/.bashrc):
```bash
eval "$(wt config shell init bash)"
```

Fish (~/.config/fish/config.fish):
```fish
wt config shell init fish | source
```

Zsh (~/.zshrc):
```zsh
eval "$(wt config shell init zsh)"
```bash

## Auto Setup

Use `wt config shell install` to automatically add to your shell config."#)]
    Init {
        /// Shell to generate code for
        #[arg(value_enum)]
        shell: Shell,

        /// Command name
        #[arg(long, default_value = DEFAULT_COMMAND_NAME)]
        command_name: String,
    },

    /// Write shell integration to config files
    #[command(after_long_help = r#"## Auto Setup

Detects existing shell config files and adds integration:
```bash
wt config shell install
```

Install for specific shell only:
```bash
wt config shell install zsh
```

Skip confirmation prompt:
```bash
wt config shell install --force
```"#)]
    Install {
        /// Shell to install (default: all)
        #[arg(value_enum)]
        shell: Option<Shell>,

        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,

        /// Command name
        #[arg(long, default_value = DEFAULT_COMMAND_NAME)]
        command_name: String,
    },

    /// Remove shell integration from config files
    #[command(after_long_help = r#"## Removal

Removes shell integration lines from config files:
```bash
wt config shell uninstall
```

Remove from specific shell only:
```bash
wt config shell uninstall zsh
```

Skip confirmation prompt:
```bash
wt config shell uninstall --force
```

## Version Tolerance

Detects various forms of the integration pattern regardless of:
- Command prefix (wt, worktree, etc.)
- Minor syntax variations between versions"#)]
    Uninstall {
        /// Shell to uninstall (default: all)
        #[arg(value_enum)]
        shell: Option<Shell>,

        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },
}

#[derive(Subcommand)]
pub enum ApprovalsCommand {
    /// Store approvals in config
    #[command(
        after_long_help = r#"Prompts for approval of all project commands and saves them to user config.

By default, shows only unapproved commands. Use `--all` to review all commands
including previously approved ones. Use `--force` to approve without prompts."#
    )]
    Ask {
        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,

        /// Show all commands
        #[arg(long)]
        all: bool,
    },

    /// Clear approved commands from config
    #[command(
        after_long_help = r#"Removes saved approvals, requiring re-approval on next command run.

By default, clears approvals for the current project. Use `--global` to clear
all approvals across all projects."#
    )]
    Clear {
        /// Clear global approvals
        #[arg(short, long)]
        global: bool,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Shell integration setup
    Shell {
        #[command(subcommand)]
        action: ConfigShellCommand,
    },

    /// Create global configuration file
    #[command(
        after_long_help = concat!(
            "Creates `~/.config/worktrunk/config.toml` with the following content:\n\n```\n",
            include_str!("../dev/config.example.toml"),
            "```"
        )
    )]
    Create,

    /// List configuration files & locations
    #[command(
        after_long_help = r#"Shows location and contents of global config (`~/.config/worktrunk/config.toml`)
and project config (`.config/wt.toml`).

If a config file doesn't exist, shows defaults that would be used."#
    )]
    List,

    /// Refresh default branch from remote
    #[command(
        after_long_help = r#"Queries the remote to determine the default branch and caches the result.

Use when the remote default branch has changed. The cached value is used by
`wt merge`, `wt list`, and other commands that reference the default branch."#
    )]
    RefreshCache,

    /// Manage branch status markers
    Status {
        #[command(subcommand)]
        action: StatusAction,
    },

    /// Manage command approvals
    #[command(after_long_help = r#"## How Approvals Work

Commands from project hooks (.config/wt.toml) and LLM configuration require
approval on first run. This prevents untrusted projects from running arbitrary
commands.

**Approval flow:**
1. Command is shown with expanded template variables
2. User approves or denies
3. Approved commands are saved to user config under `[projects."project-id"]`

**When re-approval is required:**
- Command template changes (not just variable values)
- Project ID changes (repository moves)

**Bypassing prompts:**
- `--force` flag on individual commands (e.g., `wt merge --force`)
- Useful for CI/automation where prompts aren't possible

## Examples

Pre-approve all commands for current project:
```bash
wt config approvals ask
```

Clear approvals for current project:
```bash
wt config approvals clear
```

Clear global approvals:
```bash
wt config approvals clear --global
```"#)]
    Approvals {
        #[command(subcommand)]
        action: ApprovalsCommand,
    },
}

#[derive(Subcommand)]
pub enum StatusAction {
    /// Set status emoji for branch
    #[command(
        after_long_help = r#"Sets a custom status marker that appears in `wt list` output.

Use emojis or short text to indicate work state (e.g., 🚧 WIP, ✅ ready, 🔒 blocked).
Stored in git config under `worktrunk.status.<branch>`."#
    )]
    Set {
        /// Status emoji to display
        value: String,

        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,
    },

    /// Clear status emoji
    #[command(
        after_long_help = r#"Removes status marker from branch(es). Use `*` to clear all statuses."#
    )]
    Unset {
        /// Branch or "*" for all
        #[arg(default_value = "", add = crate::completion::branch_value_completer())]
        target: String,
    },
}

/// Primitive operations (building blocks for workflows)
#[derive(Subcommand)]
pub enum StepCommand {
    /// Commit changes with LLM message
    Commit {
        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,

        /// Skip pre-commit hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,

        /// What to stage before committing [default: all]
        #[arg(long)]
        stage: Option<crate::commands::commit::StageMode>,
    },

    /// Squash commits with LLM message
    Squash {
        /// Target branch
        ///
        /// Defaults to default branch.
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,

        /// Skip pre-commit hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,

        /// What to stage before committing [default: all]
        #[arg(long)]
        stage: Option<crate::commands::commit::StageMode>,
    },

    /// Push changes to local target branch
    ///
    /// Automatically stashes non-conflicting edits in the target worktree before
    /// the push and restores them afterward so other agents' changes stay intact.
    Push {
        /// Target branch
        ///
        /// Defaults to default branch.
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,

        /// Allow merge commits
        #[arg(long)]
        allow_merge_commits: bool,
    },

    /// Rebase onto target
    Rebase {
        /// Target branch
        ///
        /// Defaults to default branch.
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,
    },

    /// Run post-create hook
    PostCreate {
        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,
    },

    /// Run post-start hook
    PostStart {
        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,
    },

    /// Run pre-commit hook
    PreCommit {
        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,
    },

    /// Run pre-merge hook
    PreMerge {
        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,
    },

    /// Run post-merge hook
    PostMerge {
        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,
    },
}

/// Experimental commands
#[derive(Subcommand)]
pub enum BetaCommand {
    /// Interactive worktree selector
    ///
    /// Preview modes (toggle with 1/2/3):
    /// - Mode 1: Working tree changes
    /// - Mode 2: History (commits not on main shown bright, commits on main dimmed)
    /// - Mode 3: Branch diff (line changes ahead of main)
    ///
    /// Note: When viewing main itself, all commits shown without dimming
    #[cfg(unix)]
    Select,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manage configuration and shell integration
    #[command(
        about = "Manage configuration and shell integration",
        after_long_help = r#"## Setup Guide

1. Set up shell integration

   ```bash
   wt config shell install
   ```

   Or manually add to your shell config:

   ```bash
   eval "$(wt config shell init bash)"
   ```

2. (Optional) Create config file

   ```bash
   wt config create
   ```

   This creates ~/.config/worktrunk/config.toml with examples.

3. (Optional) Enable LLM commit messages

   Install: `uv tool install -U llm`
   Configure: `llm keys set anthropic`
   Add to config.toml:

   ```toml
   [commit-generation]
   command = "llm"
   ```

## LLM Setup Details

For Claude:

```bash
llm install llm-anthropic
llm keys set anthropic
llm models default claude-3.5-sonnet
```

For OpenAI:

```bash
llm keys set openai
```

Use `wt config list` to view your current configuration.
Docs: <https://llm.datasette.io/> | <https://github.com/sigoden/aichat>

## Configuration Files

**Global config** (user settings):

- Location: `~/.config/worktrunk/config.toml` (or `WORKTRUNK_CONFIG_PATH`)
- Run `wt config create --help` to view documented examples

**Project config** (repository hooks):

- Location: `.config/wt.toml` in repository root
- Contains: post-create, post-start, pre-commit, pre-merge, post-merge hooks
"#
    )]
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },

    /// Primitive operations (building blocks for workflows)
    #[command(name = "step")]
    Step {
        #[command(subcommand)]
        action: StepCommand,
    },

    /// Experimental commands
    #[command(name = "beta", hide = true)]
    Beta {
        #[command(subcommand)]
        action: BetaCommand,
    },

    /// List worktrees and optionally branches
    #[command(after_long_help = "## Columns

- **Branch:** Branch name
- **Status:** Quick status symbols (see Status Symbols below)
- **HEAD±:** Uncommitted changes vs HEAD (+added -deleted lines, staged + unstaged)
- **main↕:** Commit count ahead↑/behind↓ relative to main (commits in HEAD vs main)
- **main…±** (`--full`): Line diffs in commits ahead of main (+added -deleted)
- **Path:** Worktree directory location
- **Remote⇅:** Commits ahead↑/behind↓ relative to tracking branch (e.g. `origin/branch`)
- **CI** (`--full`): CI pipeline status (tries PR/MR checks first, falls back to branch workflows)
  - `●` **passed** (green) - All checks passed
  - `●` **running** (blue) - Checks in progress
  - `●` **failed** (red) - Checks failed
  - `●` **conflicts** (yellow) - Merge conflicts with base
  - `●` **no-ci** (gray) - PR/MR or workflow found but no checks configured
  - (blank) - No PR/MR or workflow found, or `gh`/`glab` CLI unavailable
  - (dimmed) - Stale: unpushed local changes differ from PR/MR head
- **Commit:** Short commit hash (8 chars)
- **Age:** Time since last commit (relative)
- **Message:** Last commit message (truncated)

## Status Symbols

Order: `?!+»✘ ✖⚠≡∅ ↻⋈ ↑↓↕ ⇡⇣⇅ ⎇⌫⊠`

- `?` Untracked files present
- `!` Modified files (unstaged changes)
- `+` Staged files (ready to commit)
- `»` Renamed files
- `✘` Deleted files
- `✖` **Merge conflicts** - unresolved conflicts in working tree (fix before continuing)
- `⚠` **Would conflict** - merging into main would fail
- `≡` Working tree matches main (identical contents, regardless of commit history)
- `∅` No commits (no commits ahead AND no uncommitted changes)
- `↻` Rebase in progress
- `⋈` Merge in progress
- `↑` Ahead of main branch
- `↓` Behind main branch
- `↕` Diverged (both ahead and behind main)
- `⇡` Ahead of remote tracking branch
- `⇣` Behind remote tracking branch
- `⇅` Diverged (both ahead and behind remote)
- `⎇` Branch indicator (shown for branches without worktrees)
- `⌫` Prunable worktree (directory missing, can be pruned)
- `⊠` Locked worktree (protected from auto-removal)

*Rows are dimmed when no unique work (≡ matches main OR ∅ no commits).*

## JSON Output

Use `--format=json` for structured data. Each object contains two status maps
with the same fields in the same order as Status Symbols above:

**`status`** - variant names for querying:

- `working_tree`: `{untracked, modified, staged, renamed, deleted}` booleans
- `branch_state`: `\"\"` | `\"Conflicts\"` | `\"MergeTreeConflicts\"` | `\"MatchesMain\"` | `\"NoCommits\"`
- `git_operation`: `\"\"` | `\"Rebase\"` | `\"Merge\"`
- `main_divergence`: `\"\"` | `\"Ahead\"` | `\"Behind\"` | `\"Diverged\"`
- `upstream_divergence`: `\"\"` | `\"Ahead\"` | `\"Behind\"` | `\"Diverged\"`
- `user_status`: string (optional)

**`status_symbols`** - Unicode symbols for display (same fields, plus `worktree_attrs`: ⎇/⌫/⊠)

Note: `locked` and `prunable` are top-level fields on worktree objects, not in status.

**Query examples:**

```bash
# Find worktrees with conflicts
jq '.[] | select(.status.branch_state == \"Conflicts\")'

# Find worktrees with untracked files
jq '.[] | select(.status.working_tree.untracked)'

# Find worktrees in rebase or merge
jq '.[] | select(.status.git_operation != \"\")'

# Get branches ahead of main
jq '.[] | select(.status.main_divergence == \"Ahead\")'

# Find locked worktrees
jq '.[] | select(.locked != null)'
```")]
    List {
        /// Output format (table, json)
        #[arg(long, value_enum, default_value = "table", hide_possible_values = true)]
        format: OutputFormat,

        /// Include branches without worktrees
        #[arg(long)]
        branches: bool,

        /// Include remote branches
        #[arg(long)]
        remotes: bool,

        /// Show CI, conflicts, diffs
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

    /// Switch to a worktree
    #[command(after_long_help = r#"## Operation

### Switching to Existing Worktree

- If worktree exists for branch, changes directory via shell integration
- No hooks run
- No branch creation

### Creating New Worktree (`--create`)

1. Creates new branch (defaults to current default branch as base)
2. Creates worktree in configured location (default: `../{{ main_worktree }}.{{ branch }}`)
3. Runs post-create hooks sequentially (blocking)
4. Shows success message
5. Spawns post-start hooks in background (non-blocking)
6. Changes directory to new worktree via shell integration

## Hooks

### post-create (sequential, blocking)

- Run after worktree creation, before success message
- Typically: `npm install`, `cargo build`, setup tasks
- Failures block the operation
- Skip with `--no-verify`

### post-start (parallel, background)

- Spawned after success message shown
- Typically: dev servers, file watchers, editors
- Run in background, failures logged but don't block
- Logs: `.git/wt-logs/{branch}-post-start-{name}.log`
- Skip with `--no-verify`

**Template variables:** `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`

**Security:** Commands from project hooks require approval on first run.
Approvals are saved to user config. Use `--force` to bypass prompts.
See `wt config approvals --help`.

## Examples

Switch to existing worktree:

```bash
wt switch feature-branch
```

Create new worktree from main:

```bash
wt switch --create new-feature
```

Switch to previous worktree:

```bash
wt switch -
```

Create from specific base:

```bash
wt switch --create hotfix --base production
```

Create and run command:

```bash
wt switch --create docs --execute "code ."
```

Skip hooks during creation:

```bash
wt switch --create temp --no-verify
```

## Shortcuts

Use `@` for current HEAD, `-` for previous, `^` for main:

```bash
wt switch @                              # Switch to current branch's worktree
wt switch -                              # Switch to previous worktree
wt switch --create new-feature --base=^  # Branch from main (default)
wt switch --create bugfix --base=@       # Branch from current HEAD
wt remove @                              # Remove current worktree
```"#)]
    Switch {
        /// Branch, path, '@' (HEAD), '-' (previous), or '^' (main)
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
        #[arg(short = 'x', long)]
        execute: Option<String>,

        /// Skip approval prompts
        #[arg(short = 'f', long)]
        force: bool,

        /// Skip all project hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,
    },

    /// Remove worktree and branch
    #[command(after_long_help = r#"## Operation

Removes worktree directory, git metadata, and branch. Requires clean working tree.

### No arguments (remove current)

- Removes current worktree and switches to main worktree
- In main worktree: switches to default branch

### By name (remove specific)

- Removes specified worktree(s) and branches
- Current worktree removed last (switches to main first)

### Background removal (default)

- Returns immediately so you can continue working
- Logs: `.git/wt-logs/{branch}-remove.log`
- Use `--no-background` for foreground (blocking)

### Cleanup

Stops any git fsmonitor daemon for the worktree before removal. This prevents orphaned processes when using builtin fsmonitor (`core.fsmonitor=true`). No effect on Watchman users.

## Examples

Remove current worktree and branch:

```bash
wt remove
```

Remove specific worktree and branch:

```bash
wt remove feature-branch
```

Remove worktree but keep branch:

```bash
wt remove --no-delete-branch feature-branch
```

Remove multiple worktrees:

```bash
wt remove old-feature another-branch
```

Remove in foreground (blocking):

```bash
wt remove --no-background feature-branch
```

Switch to default in main:

```bash
wt remove  # (when already in main worktree)
```"#)]
    Remove {
        /// Worktree or branch (@ for current)
        #[arg(add = crate::completion::worktree_branch_completer())]
        worktrees: Vec<String>,

        /// Keep branch after removal
        #[arg(long = "no-delete-branch", action = clap::ArgAction::SetFalse, default_value_t = true)]
        delete_branch: bool,

        /// Delete unmerged branches
        #[arg(short = 'D', long = "force-delete")]
        force_delete: bool,

        /// Run removal in foreground
        #[arg(long = "no-background", action = clap::ArgAction::SetFalse, default_value_t = true)]
        background: bool,
    },

    /// Merge worktree into target branch
    #[command(after_long_help = r#"## Operation

Commit → Squash → Rebase → Pre-merge hooks → Push → Cleanup → Post-merge hooks

### Commit

Uncommitted changes are staged and committed with LLM message.
Use `--stage=tracked` to stage only tracked files, or `--stage=none` to commit only what's already staged.

### Squash

Multiple commits are squashed into one with LLM message.
Skip with `--no-squash`. Safety backup: `git reflog show refs/wt-backup/<branch>`

### Rebase

Branch is rebased onto target. Conflicts abort the merge immediately.

### Hooks

Pre-merge commands run after rebase (failures abort). Post-merge commands
run after cleanup (failures logged). Skip all with `--no-verify`.

### Push

Fast-forward push to local target branch. Non-fast-forward pushes are rejected.

### Cleanup

Worktree and branch are removed. Skip with `--no-remove`.

**Template variables:** `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`, `{{ target }}`

**Security:** Commands from project hooks require approval on first run.
Approvals are saved to user config. Use `--force` to bypass prompts.
See `wt config approvals --help`.

## Examples

Basic merge to main:

```bash
wt merge
```

Merge without squashing:

```bash
wt merge --no-squash
```

Keep worktree after merging:

```bash
wt merge --no-remove
```

Skip all hooks:

```bash
wt merge --no-verify
```"#)]
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

        /// Force commit, squash, and rebase
        #[arg(long, overrides_with = "no_commit", hide = true)]
        commit: bool,

        /// Skip commit, squash, and rebase
        #[arg(long = "no-commit", overrides_with = "commit")]
        no_commit: bool,

        /// Force worktree removal after merge
        #[arg(long, overrides_with = "no_remove", hide = true)]
        remove: bool,

        /// Keep worktree after merge
        #[arg(long = "no-remove", overrides_with = "remove")]
        no_remove: bool,

        /// Force running project hooks
        #[arg(long, overrides_with = "no_verify", hide = true)]
        verify: bool,

        /// Skip all project hooks
        #[arg(long = "no-verify", overrides_with = "verify")]
        no_verify: bool,

        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,

        /// What to stage before committing [default: all]
        #[arg(long)]
        stage: Option<crate::commands::commit::StageMode>,
    },
}
