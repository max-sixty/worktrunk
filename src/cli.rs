use clap::builder::styling::{AnsiColor, Color, Styles};
use clap::{Parser, Subcommand};
use std::sync::OnceLock;
use worktrunk::HookType;

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
pub struct Cli {
    /// Change working directory
    #[arg(short = 'C', global = true, value_name = "path")]
    pub directory: Option<std::path::PathBuf>,

    /// Show commands and debug info
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,

    /// Use internal mode (outputs directives for shell wrapper)
    #[arg(long, global = true, hide = true)]
    pub internal: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Initialize global configuration file with examples
    Init,
    /// List configuration files & locations
    List,
    /// Refresh default branch from remote
    RefreshCache,
    /// Configure shell integration
    #[command(long_about = r#"Configure shell by writing to config files

This command automatically adds the appropriate integration line to your shell's config file.
It supports Bash, Zsh, Fish, Nushell, PowerShell, Elvish, Xonsh, and Oil.

The integration enables 'wt switch' to change directories and 'wt remove' to return to the
previous location.

MANUAL SETUP (if you prefer):

Add one line to your shell config:

Bash (~/.bashrc):
  eval "$(wt init bash)"

Fish (~/.config/fish/config.fish):
  wt init fish | source

Zsh (~/.zshrc):
  eval "$(wt init zsh)"

Nushell (~/.config/nushell/env.nu):
  wt init nushell | save -f ~/.cache/wt-init.nu

Then add to ~/.config/nushell/config.nu:
  source ~/.cache/wt-init.nu

PowerShell (profile):
  wt init powershell | Out-String | Invoke-Expression

Elvish (~/.config/elvish/rc.elv):
  eval (wt init elvish | slurp)

Xonsh (~/.xonshrc):
  execx($(wt init xonsh))

Oil Shell (~/.config/oil/oshrc):
  eval "$(wt init oil)""#)]
    Shell {
        /// Shell to configure
        #[arg(long, value_enum)]
        shell: Option<Shell>,

        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,

        /// Command name
        #[arg(long, default_value = DEFAULT_COMMAND_NAME)]
        command_name: String,
    },

    /// Manage branch status markers
    Status {
        #[command(subcommand)]
        action: StatusAction,
    },
}

#[derive(Subcommand)]
pub enum StatusAction {
    /// Set status emoji for branch
    Set {
        /// Status emoji to display
        value: String,

        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,
    },

    /// Clear status emoji
    Unset {
        /// Branch or "*" for all
        #[arg(default_value = "", add = crate::completion::branch_value_completer())]
        target: String,
    },
}

#[derive(Subcommand)]
pub enum StandaloneCommand {
    /// Run project hook
    RunHook {
        /// Hook type to run
        hook_type: HookType,

        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,
    },

    /// Commit changes with LLM-generated message
    Commit {
        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,

        /// Skip pre-commit hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,
    },

    /// Squash commits with LLM-generated message
    Squash {
        /// Target branch (defaults to default branch)
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,

        /// Skip pre-commit hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,
    },

    /// Push changes to target branch
    ///
    /// Automatically stashes non-conflicting edits in the target worktree before
    /// the push and restores them afterward so other agents' changes stay intact.
    Push {
        /// Target branch (defaults to default branch)
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,

        /// Allow merge commits
        #[arg(long)]
        allow_merge_commits: bool,
    },

    /// Rebase onto target
    Rebase {
        /// Target branch (defaults to default branch)
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,
    },

    /// Store approvals in config
    AskApprovals {
        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,

        /// Show all commands
        #[arg(long)]
        all: bool,
    },

    /// Clear approved commands from config
    ClearApprovals {
        /// Clear global approvals
        #[arg(short, long)]
        global: bool,
    },

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
    /// Generate shell integration code
    Init {
        /// Shell to generate code for
        shell: Shell,

        /// Command name
        #[arg(long, default_value = DEFAULT_COMMAND_NAME)]
        command_name: String,
    },

    /// Manage configuration
    #[command(
        about = "Manage configuration. For AI commit setup, run: `wt config --help` (see 'LLM SETUP GUIDE').",
        after_long_help = r#"LLM SETUP GUIDE:
Enable AI-generated commit messages

1. Install an LLM tool (llm, aichat)

   uv tool install -U llm

2. Configure a model

For Claude:
   llm install llm-anthropic
   llm keys set anthropic
   # Paste your API key from: https://console.anthropic.com/settings/keys
   llm models default claude-3.5-sonnet

For OpenAI:
   llm keys set openai
   # Paste your API key from: https://platform.openai.com/api-keys

3. Test it works

   llm "say hello"

4. Configure worktrunk

Add to ~/.config/worktrunk/config.toml:

   [commit-generation]
   command = "llm"

Use 'wt config init' to create the config file if it doesn't exist
Use 'wt config list' to view your current configuration
Docs: https://llm.datasette.io/ | https://github.com/sigoden/aichat
"#
    )]
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },

    /// Development and testing utilities
    #[command(name = "beta", hide = true)]
    Standalone {
        #[command(subcommand)]
        action: StandaloneCommand,
    },

    /// List worktrees and optionally branches
    #[command(after_help = "OPERATION

Displays worktrees in a table format with status information, commit details, and optional branch listings.

- By default: Shows only worktrees
- With --branches: Includes branches without worktrees
- With --full: Adds CI status, conflict detection, and detailed diffs
- With --format=json: Outputs structured JSON for scripting

COLUMNS

  Branch: Branch name
  Status: Quick status symbols (see STATUS SYMBOLS below)
  HEAD±: Uncommitted changes vs HEAD (+added -deleted lines, staged + unstaged)
  main↕: Commit count ahead↑/behind↓ relative to main (commits in HEAD vs main)
  main…± (--full): Line diffs in commits ahead of main (+added -deleted)
  Path: Worktree directory location
  Remote⇅: Commits ahead↑/behind↓ relative to tracking branch (e.g. origin/branch)
  CI (--full): CI pipeline status (tries PR/MR checks first, falls back to branch workflows)
    \x1b[32m●\x1b[0m passed (green) - All checks passed
    \x1b[34m●\x1b[0m running (blue) - Checks in progress
    \x1b[31m●\x1b[0m failed (red) - Checks failed
    \x1b[33m●\x1b[0m conflicts (yellow) - Merge conflicts with base
    \x1b[90m●\x1b[0m no-ci (gray) - PR/MR or workflow found but no checks configured
    (blank) - No PR/MR or workflow found, or gh/glab CLI unavailable
    (dimmed) - Stale: unpushed local changes differ from PR/MR head
  Commit: Short commit hash (8 chars)
  Age: Time since last commit (relative)
  Message: Last commit message (truncated)

STATUS SYMBOLS (order: =≠ ≡∅ ↻⋈ ⊠⚠ ↑↓ ⇡⇣ ?!+»✘):
  ·  Branch without worktree (no working directory to check)
  \x1b[31m=\x1b[0m  Merge conflicts (unmerged paths in working tree)
  \x1b[33m≠\x1b[0m  Potential conflicts with main (--full only, detected via git merge-tree)
  ≡  Working tree matches main (identical contents, regardless of commit history)
  ∅  No commits (no commits ahead AND no uncommitted changes)
  ↻  Rebase in progress
  ⋈  Merge in progress
  ⊠  Locked worktree
  ⚠  Prunable worktree
  ↑  Ahead of main branch
  ↓  Behind main branch
  ⇡  Ahead of remote tracking branch
  ⇣  Behind remote tracking branch
  ?  Untracked files present
  !  Modified files (unstaged changes)
  +  Staged files (ready to commit)
  »  Renamed files
  ✘  Deleted files

Rows are dimmed when no unique work (≡ matches main OR ∅ no commits).")]
    List {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,

        /// Include branches without worktrees
        #[arg(long)]
        branches: bool,

        /// Show CI, conflicts, and full diffs
        ///
        /// Adds columns: CI (pipeline status), main…± (line diffs).
        /// Enables conflict detection (shows "=" symbol in Status column).
        /// Requires network requests and git merge-tree operations.
        #[arg(long, verbatim_doc_comment)]
        full: bool,

        /// Enable progressive rendering (show rows as data arrives)
        ///
        /// When enabled, worktree names appear immediately and details fill in
        /// progressively. Default: auto (enabled for TTY, disabled for pipes).
        #[arg(long, verbatim_doc_comment, conflicts_with = "no_progressive")]
        progressive: bool,

        /// Disable progressive rendering (collect all data before displaying)
        #[arg(long)]
        no_progressive: bool,
    },

    /// Switch to a worktree
    #[command(after_help = r#"OPERATION

Switching to Existing Worktree:
  - If worktree exists for branch, changes directory via shell integration
  - No hooks run
  - No branch creation

Creating New Worktree (--create):
  1. Creates new branch (defaults to current default branch as base)
  2. Creates worktree in configured location (default: ../{{ main_worktree }}.{{ branch }})
  3. Runs post-create hooks sequentially (blocking)
  4. Shows success message
  5. Spawns post-start hooks in background (non-blocking)
  6. Changes directory to new worktree via shell integration

HOOKS

post-create (sequential, blocking):
  - Run after worktree creation, before success message
  - Typically: npm install, cargo build, setup tasks
  - Failures block the operation
  - Skip with --no-verify

post-start (parallel, background):
  - Spawned after success message shown
  - Typically: dev servers, file watchers, editors
  - Run in background, failures logged but don't block
  - Logs: .git/wt-logs/{branch}-post-start-{name}.log
  - Skip with --no-verify

EXAMPLES

Switch to existing worktree:
  wt switch feature-branch

Create new worktree from main:
  wt switch --create new-feature

Switch to previous worktree:
  wt switch -

Create from specific base:
  wt switch --create hotfix --base production

Create and run command:
  wt switch --create docs --execute "code ."

Skip hooks during creation:
  wt switch --create temp --no-verify

SHORTCUTS

Use '@' for current HEAD, '-' for previous, '^' for main:
  wt switch @                              # Switch to current branch's worktree
  wt switch -                              # Switch to previous worktree
  wt switch --create new-feature --base=^  # Branch from main (default)
  wt switch --create bugfix --base=@       # Branch from current HEAD
  wt remove @                              # Remove current worktree"#)]
    Switch {
        /// Branch, path, '@' (HEAD), '-' (previous), or '^' (main)
        #[arg(add = crate::completion::worktree_branch_completer())]
        branch: String,

        /// Create a new branch
        #[arg(short = 'c', long)]
        create: bool,

        /// Base branch (defaults to default branch)
        #[arg(short = 'b', long, add = crate::completion::branch_value_completer())]
        base: Option<String>,

        /// Execute command after switching
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
    #[command(after_help = r#"OPERATION

Remove Current Worktree (no arguments):
  - Requires clean working tree (no uncommitted changes)
  - If in worktree: removes it and switches to main worktree
  - If in main worktree: switches to default branch (e.g., main)
  - If already on default branch in main: does nothing

Remove Specific Worktree (by name):
  - Requires target worktree has clean working tree
  - Removes specified worktree(s) and associated branches
  - If removing current worktree, switches to main first
  - Can remove multiple worktrees in one command

Remove Multiple Worktrees:
  - When removing multiple, current worktree is removed last
  - Prevents deleting directory you're currently in
  - Each worktree must have clean working tree

Removal Process (by default):
  1. Validates worktree has no uncommitted changes
  2. Changes directory (if removing current worktree)
  3. Spawns background removal process (non-blocking)
     - Directory deletion happens in background
     - Git worktree metadata removed in background
     - Branch deletion in background (uses git branch -d, safe delete)
     - Logs: .git/wt-logs/{branch}-remove.log
  4. Returns immediately so you can continue working
     - Use --no-background for foreground removal (blocking)

EXAMPLES

Remove current worktree and branch:
  wt remove

Remove specific worktree and branch:
  wt remove feature-branch

Remove worktree but keep branch:
  wt remove --no-delete-branch feature-branch

Remove multiple worktrees:
  wt remove old-feature another-branch

Remove in foreground (blocking):
  wt remove --no-background feature-branch

Switch to default in main:
  wt remove  # (when already in main worktree)"#)]
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
    #[command(long_about = r#"Merge worktree into target branch

OPERATION

The merge operation follows a strict order designed for fail-fast execution:

1. Validate branches
   Verifies current branch exists (not detached HEAD) and determines target branch
   (defaults to repository's default branch).

2. Auto-commit uncommitted changes
   If working tree has uncommitted changes, stages changes and commits with LLM-generated
   message. By default stages all changes (git add -A). Use --tracked-only to stage only
   tracked files (git add -u).

3. Squash commits (default)
   By default, counts commits since merge base with target branch. When multiple
   commits exist, squashes them into one with LLM-generated message. Skip squashing
   with --no-squash.

   A safety backup is created before squashing if there are working tree changes.
   Recover with: git reflog show refs/wt-backup/<branch>

4. Rebase onto target
   Rebases current branch onto target branch. Detects conflicts and aborts if found.
   This fails fast before running expensive checks.

5. Run pre-merge commands
   Runs commands from project config's [pre-merge-command] after rebase completes.
   These receive {{ target }} placeholder for the target branch. Commands run sequentially
   and any failure aborts the merge immediately. Skip with --no-verify.

6. Push to target
   Fast-forward pushes to target branch. Rejects non-fast-forward pushes (ensures
   linear history). Temporarily stashes non-conflicting local edits in the target
   worktree so they don't block the push, then restores them after success.

7. Clean up worktree and branch
   Removes current worktree, deletes the branch, and switches main worktree to target
   branch if needed. Skip removal with --no-remove.

8. Run post-merge commands
   Runs commands from project config's [post-merge-command] in the destination worktree
   after cleanup. These receive {{ target }} placeholder. Commands run sequentially,
   failures are logged but don't abort. Skip with --no-verify.

HOOKS

The --no-verify flag skips all project hooks:
- Pre-commit hooks (before committing working tree changes)
- Pre-merge hooks (after rebase, before push)
- Post-merge hooks (after cleanup)

EXAMPLES

Basic merge to main:
  wt merge

Merge without squashing:
  wt merge --no-squash

Keep worktree after merging:
  wt merge --no-remove

Skip all hooks:
  wt merge --no-verify"#)]
    Merge {
        /// Target branch (defaults to default branch)
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,

        /// Skip commit squashing
        #[arg(long = "no-squash", action = clap::ArgAction::SetFalse, default_value_t = true)]
        squash: bool,

        /// Skip commit, squash, and rebase
        #[arg(long = "no-commit", action = clap::ArgAction::SetFalse, default_value_t = true)]
        commit: bool,

        /// Keep worktree after merge
        #[arg(long = "no-remove", action = clap::ArgAction::SetFalse, default_value_t = true)]
        remove: bool,

        /// Skip all project hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,

        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,

        /// Stage tracked files only
        #[arg(long)]
        tracked_only: bool,
    },
}
