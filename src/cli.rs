use clap::{ArgAction, Parser, Subcommand};
use std::sync::OnceLock;
use worktrunk::HookType;

use crate::commands::Shell;

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
    /// Machine-readable JSON with display fields (includes styled unicode for rendering)
    Json,
}

#[derive(Parser)]
#[command(name = "wt")]
#[command(about = "Git worktree management", long_about = None)]
#[command(version = version_str())]
#[command(disable_help_subcommand = true)]
pub struct Cli {
    /// Enable verbose output (show git commands and debug info)
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
    /// List all configuration files and their locations
    List,
    /// Refresh the cached default branch by querying the remote
    RefreshCache,
    /// Configure shell by writing to config files
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
        /// Specific shell to configure (default: all shells with existing config files)
        #[arg(long, value_enum)]
        shell: Option<Shell>,

        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,

        /// Command name to use (defaults to 'wt')
        #[arg(long, default_value = DEFAULT_COMMAND_NAME)]
        command_name: String,
    },
}

#[derive(Subcommand)]
pub enum StandaloneCommand {
    /// Run a project hook for testing
    RunHook {
        /// Hook type to run
        hook_type: HookType,

        /// Auto-approve project commands without saving approvals.
        #[arg(short, long)]
        force: bool,
    },

    /// Commit changes with LLM-generated message
    Commit {
        /// Auto-approve project commands without saving approvals.
        #[arg(short, long)]
        force: bool,

        /// Skip pre-commit project hooks.
        #[arg(long)]
        no_verify: bool,
    },

    /// Squash commits with LLM-generated message
    Squash {
        /// Target branch to squash against (defaults to default branch)
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,

        /// Auto-approve project commands without saving approvals.
        #[arg(short, long)]
        force: bool,

        /// Skip pre-commit project hooks
        #[arg(long)]
        no_verify: bool,
    },

    /// Push changes to target branch
    ///
    /// Automatically stashes non-conflicting edits in the target worktree before
    /// the push and restores them afterward so other agents' changes stay intact.
    Push {
        /// Target branch (defaults to default branch)
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,

        /// Allow pushing merge commits (non-linear history)
        #[arg(long)]
        allow_merge_commits: bool,
    },

    /// Rebase current branch onto target branch
    Rebase {
        /// Target branch to rebase onto (defaults to default branch)
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,
    },

    /// Approve commands in the project config (shows unapproved by default)
    AskApprovals {
        /// Auto-approve project commands without saving approvals.
        #[arg(short, long)]
        force: bool,

        /// Show all commands including already-approved ones
        #[arg(long)]
        all: bool,
    },

    /// Clear approved commands from config
    ClearApprovals {
        /// Clear approvals from global config (all projects)
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

        /// Command name to use (defaults to 'wt')
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
    #[command(after_help = r#"COLUMNS:
  Branch: Branch name
  Status: Quick status symbols (see STATUS SYMBOLS below)
  HEAD±: Uncommitted changes vs HEAD (+added -deleted lines, staged + unstaged)
  main↕: Commit count ahead↑/behind↓ relative to main (commits in HEAD vs main)
  main…± (--full): Line diffs in commits ahead of main (+added -deleted)
  Path: Worktree directory location
  Remote⇅: Commits ahead↑/behind↓ relative to tracking branch (e.g. origin/branch)
  CI (--full): CI pipeline status (tries PR/MR checks first, falls back to branch workflows)
    ● passed (green) - All checks passed
    ● running (blue) - Checks in progress
    ● failed (red) - Checks failed
    ● conflicts (yellow) - Merge conflicts with base
    ● no-ci (gray) - PR/MR or workflow found but no checks configured
    (blank) - No PR/MR or workflow found, or gh/glab CLI unavailable
    (dimmed) - Stale: unpushed local changes differ from PR/MR head
  Commit: Short commit hash (8 chars)
  Age: Time since last commit (relative)
  Message: Last commit message (truncated)

STATUS SYMBOLS (order: = ≡∅ ↻⋈ ◇⊠⚠ ↑↓ ⇡⇣ ?!+»✘):
  ·  Branch without worktree (no working directory to check)
  =  Merge conflicts (unmerged paths in working tree)
  ≡  Working tree matches main (identical contents, regardless of commit history)
  ∅  No commits (no commits ahead AND no uncommitted changes)
  ↻  Rebase in progress
  ⋈  Merge in progress
  ◇  Bare worktree (no working directory)
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

Rows are dimmed when no unique work (≡ matches main OR ∅ no commits)."#)]
    List {
        /// Output format
        #[arg(long, value_enum, default_value = "table")]
        format: OutputFormat,

        /// Include branches without worktrees
        #[arg(long)]
        branches: bool,

        /// Show CI status, conflict detection, and complete diff statistics
        ///
        /// Adds columns: CI (pipeline status), main…± (line diffs).
        /// Enables conflict detection (shows "=" symbol in Status column).
        /// Requires network requests and git merge-tree operations.
        #[arg(long, verbatim_doc_comment)]
        full: bool,
    },

    /// Switch to a worktree
    #[command(after_help = r#"BEHAVIOR:

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

HOOKS:

post-create (sequential, blocking):
  - Run after worktree creation, before success message
  - Typically: npm install, cargo build, setup tasks
  - Failures block the operation
  - Skip with --no-verify

post-start (parallel, background):
  - Spawned after success message shown
  - Typically: dev servers, file watchers, editors
  - Run in background, failures logged but don't block
  - Skip with --no-verify

EXAMPLES:

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

SHORTCUTS:

Use '@' to refer to your current HEAD (following git's convention):
  wt switch @                              # Switch to current branch's worktree
  wt switch --create new-feature --base=@  # Branch from current HEAD
  wt remove @                              # Remove current worktree"#)]
    Switch {
        /// Branch name, worktree path, '@' for current HEAD, or '-' for previous branch
        #[arg(add = crate::completion::worktree_branch_completer())]
        branch: String,

        /// Create a new branch
        #[arg(short = 'c', long)]
        create: bool,

        /// Base branch to create from (only with --create). Use '@' for current HEAD
        #[arg(short = 'b', long, add = crate::completion::branch_value_completer())]
        base: Option<String>,

        /// Execute command after switching
        #[arg(short = 'x', long)]
        execute: Option<String>,

        /// Auto-approve project commands without saving approvals.
        #[arg(short = 'f', long)]
        force: bool,

        /// Skip all project hooks: post-create and post-start.
        #[arg(long)]
        no_verify: bool,
    },

    /// Finish current worktree, returning to primary if current
    #[command(after_help = r#"BEHAVIOR:

Remove Current Worktree (no arguments):
  - Requires clean working tree (no uncommitted changes)
  - If in worktree: removes it and switches to primary worktree
  - If in primary worktree: switches to default branch (e.g., main)
  - If already on default branch in primary: does nothing

Remove Specific Worktree (by name):
  - Requires target worktree has clean working tree
  - Removes specified worktree(s) and associated branches
  - If removing current worktree, switches to primary first
  - Can remove multiple worktrees in one command

Remove Multiple Worktrees:
  - When removing multiple, current worktree is removed last
  - Prevents deleting directory you're currently in
  - Each worktree must have clean working tree

CLEANUP:

When removing a worktree (by default):
  1. Validates worktree has no uncommitted changes
  2. Changes directory (if removing current worktree)
  3. Deletes worktree directory
  4. Removes git worktree metadata
  5. Deletes associated branch (uses git branch -d, safe delete)
     - If branch has unmerged commits, shows warning but continues
     - Use --no-delete-branch to skip branch deletion

EXAMPLES:

Remove current worktree and branch:
  wt remove

Remove specific worktree and branch:
  wt remove feature-branch

Remove worktree but keep branch:
  wt remove --no-delete-branch feature-branch

Remove multiple worktrees:
  wt remove old-feature another-branch

Switch to default in primary:
  wt remove  # (when already in primary worktree)"#)]
    Remove {
        /// Worktree names or branches to remove (use '@' for current, defaults to current if none specified)
        #[arg(add = crate::completion::worktree_branch_completer())]
        worktrees: Vec<String>,

        /// Don't delete the branch after removing the worktree (by default, branches are deleted)
        #[arg(long = "no-delete-branch")]
        no_delete_branch: bool,
    },

    /// Merge worktree into target branch
    #[command(long_about = r#"Merge worktree into target branch

LIFECYCLE

The merge operation follows a strict order designed for fail-fast execution:

1. Validate branches
   Verifies current branch exists (not detached HEAD) and determines target branch
   (defaults to repository's default branch).

2. Auto-commit uncommitted changes
   If working tree has uncommitted changes, stages all changes (git add -A) and commits
   with LLM-generated message.

3. Squash commits (default)
   By default, counts commits since merge base with target branch. When multiple
   commits exist, squashes them into one with LLM-generated message. Skip squashing
   with --no-squash.

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
   Removes current worktree, deletes the branch, and switches primary worktree to target
   branch if needed. Skip removal with --no-remove.

EXAMPLES

Basic merge to main:
  wt merge

Merge without squashing:
  wt merge --no-squash

Keep worktree after merging:
  wt merge --no-remove

Skip pre-merge commands:
  wt merge --no-verify"#)]
    Merge {
        /// Target branch to merge into (defaults to default branch)
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,

        /// Disable squashing commits (by default, commits are squashed into one before merging)
        #[arg(long = "no-squash", action = ArgAction::SetFalse, default_value_t = true)]
        squash_enabled: bool,

        /// Push commits as-is without transformations (requires clean tree; implies --no-squash, --no-remove, and skips rebase)
        #[arg(long)]
        no_commit: bool,

        /// Keep the worktree after merging.
        #[arg(long = "no-remove")]
        no_remove: bool,

        /// Skip pre-merge project hooks.
        #[arg(long)]
        no_verify: bool,

        /// Auto-approve project commands without saving approvals.
        #[arg(short, long)]
        force: bool,

        /// Only stage tracked files (git add -u) instead of all files (git add -A)
        #[arg(long)]
        tracked_only: bool,
    },
}
