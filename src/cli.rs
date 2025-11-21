use clap::builder::styling::{AnsiColor, Color, Styles};
use clap::{Command, CommandFactory, Parser, Subcommand};
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

/// Help template for commands without subcommands
const HELP_TEMPLATE: &str = "\
{before-help}{name} - {about-with-newline}\
Usage: {usage}

Options:
{options}{after-help}";

/// Help template for commands with subcommands
const HELP_TEMPLATE_WITH_SUBCOMMANDS: &str = "\
{before-help}{name} - {about-with-newline}\
Usage: {usage}

Commands:
{subcommands}

Options:
{options}{after-help}";

/// Build a clap Command for Cli with the shared help template applied recursively.
pub fn build_command() -> Command {
    let cmd = Cli::command();
    apply_help_template_recursive(cmd)
}

fn apply_help_template_recursive(mut cmd: Command) -> Command {
    let template = if cmd.get_subcommands().next().is_some() {
        HELP_TEMPLATE_WITH_SUBCOMMANDS
    } else {
        HELP_TEMPLATE
    };
    cmd = cmd.help_template(template);

    for sub in cmd.get_subcommands_mut() {
        let sub_cmd = std::mem::take(sub);
        let sub_cmd = apply_help_template_recursive(sub_cmd);
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
#[command(after_long_help = r#"## GLOBAL OPTIONS

**-C <path>**: Run as if started in `<path>` instead of current directory.

**--config <path>**: Override user config file location. Without this flag,
config is loaded from (in order of precedence):
1. `WORKTRUNK_CONFIG_PATH` environment variable
2. `~/.config/worktrunk/config.toml` (Linux/macOS) or `%APPDATA%\worktrunk\config.toml` (Windows)"#)]
pub struct Cli {
    /// Change working directory
    #[arg(short = 'C', global = true, value_name = "path", display_order = 100)]
    pub directory: Option<std::path::PathBuf>,

    /// User config file path
    #[arg(long, global = true, value_name = "path", display_order = 101)]
    pub config: Option<std::path::PathBuf>,

    /// Show commands and debug info
    #[arg(long, short = 'v', global = true, display_order = 102)]
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
    #[command(after_long_help = r#"MANUAL SETUP:

Add one line to your shell config:

Bash (~/.bashrc):
  eval "$(wt config shell init bash)"

Fish (~/.config/fish/config.fish):
  wt config shell init fish | source

Zsh (~/.zshrc):
  eval "$(wt config shell init zsh)"

AUTO SETUP:

Use 'wt config shell install' to automatically add to your shell config."#)]
    Init {
        /// Shell to generate code for
        #[arg(value_enum)]
        shell: Shell,

        /// Command name
        #[arg(long, default_value = DEFAULT_COMMAND_NAME)]
        command_name: String,
    },

    /// Write shell integration to config files
    #[command(after_long_help = r#"AUTO SETUP:

Detects existing shell config files and adds integration:
  wt config shell install

Install for specific shell only:
  wt config shell install zsh

Skip confirmation prompt:
  wt config shell install --force"#)]
    Install {
        /// Shell to install (default: auto-detect)
        #[arg(value_enum)]
        shell: Option<Shell>,

        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,

        /// Command name
        #[arg(long, default_value = DEFAULT_COMMAND_NAME)]
        command_name: String,
    },
}

#[derive(Subcommand)]
pub enum ApprovalsCommand {
    /// Store approvals in config
    Ask {
        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,

        /// Show all commands
        #[arg(long)]
        all: bool,
    },

    /// Clear approved commands from config
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
    Create,

    /// List configuration files & locations
    List,

    /// Refresh default branch from remote
    RefreshCache,

    /// Manage branch status markers
    Status {
        #[command(subcommand)]
        action: StatusAction,
    },

    /// Manage command approvals
    Approvals {
        #[command(subcommand)]
        action: ApprovalsCommand,
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

    /// Push changes to local target branch
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
        after_long_help = r#"SETUP GUIDE:

1. Set up shell integration

   wt config shell install

   Or manually add to your shell config:
   eval "$(wt config shell init bash)"

2. (Optional) Create config file

   wt config create

   This creates ~/.config/worktrunk/config.toml with examples.

3. (Optional) Enable LLM commit messages

   Install: uv tool install -U llm
   Configure: llm keys set anthropic
   Add to config.toml:
     [commit-generation]
     command = "llm"

LLM SETUP DETAILS:

For Claude:
   llm install llm-anthropic
   llm keys set anthropic
   llm models default claude-3.5-sonnet

For OpenAI:
   llm keys set openai

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
    #[command(after_long_help = "## COLUMNS

- **Branch:** Branch name
- **Status:** Quick status symbols (see STATUS SYMBOLS below)
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

## STATUS SYMBOLS

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

## JSON OUTPUT

Use `--format=json` for structured data. Each object contains two status maps
with the same fields in the same order as STATUS SYMBOLS above:

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

  # Find worktrees with conflicts
  jq '.[] | select(.status.branch_state == \"Conflicts\")'

  # Find worktrees with untracked files
  jq '.[] | select(.status.working_tree.untracked)'

  # Find worktrees in rebase or merge
  jq '.[] | select(.status.git_operation != \"\")'

  # Get branches ahead of main
  jq '.[] | select(.status.main_divergence == \"Ahead\")'

  # Find locked worktrees
  jq '.[] | select(.locked != null)'")]
    List {
        /// Output format (table, json)
        #[arg(long, value_enum, default_value = "table", hide_possible_values = true)]
        format: OutputFormat,

        /// Include branches without worktrees
        #[arg(long)]
        branches: bool,

        /// Include remote branches from primary remote
        #[arg(long)]
        remotes: bool,

        /// Show CI, conflicts, diffs
        #[arg(long)]
        full: bool,

        /// Force progressive (or --no-progressive)
        #[arg(long, overrides_with = "no_progressive")]
        progressive: bool,

        /// Force buffered rendering
        #[arg(long = "no-progressive", overrides_with = "progressive", hide = true)]
        no_progressive: bool,
    },

    /// Switch to a worktree
    #[command(after_long_help = r#"## OPERATION

**Switching to Existing Worktree:**
- If worktree exists for branch, changes directory via shell integration
- No hooks run
- No branch creation

**Creating New Worktree** (`--create`):
1. Creates new branch (defaults to current default branch as base)
2. Creates worktree in configured location (default: `../{{ main_worktree }}.{{ branch }}`)
3. Runs post-create hooks sequentially (blocking)
4. Shows success message
5. Spawns post-start hooks in background (non-blocking)
6. Changes directory to new worktree via shell integration

## HOOKS

**post-create** (sequential, blocking):
- Run after worktree creation, before success message
- Typically: `npm install`, `cargo build`, setup tasks
- Failures block the operation
- Skip with `--no-verify`

**post-start** (parallel, background):
- Spawned after success message shown
- Typically: dev servers, file watchers, editors
- Run in background, failures logged but don't block
- Logs: `.git/wt-logs/{branch}-post-start-{name}.log`
- Skip with `--no-verify`

## EXAMPLES

Switch to existing worktree:
```
wt switch feature-branch
```

Create new worktree from main:
```
wt switch --create new-feature
```

Switch to previous worktree:
```
wt switch -
```

Create from specific base:
```
wt switch --create hotfix --base production
```

Create and run command:
```
wt switch --create docs --execute "code ."
```

Skip hooks during creation:
```
wt switch --create temp --no-verify
```

## SHORTCUTS

Use `@` for current HEAD, `-` for previous, `^` for main:
```
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
    #[command(after_long_help = r#"## OPERATION

Removes worktree directory, git metadata, and branch. Requires clean working tree.

**No arguments** (remove current):
- Removes current worktree and switches to main worktree
- In main worktree: switches to default branch

**By name** (remove specific):
- Removes specified worktree(s) and branches
- Current worktree removed last (switches to main first)

**Background removal** (default):
- Returns immediately so you can continue working
- Logs: `.git/wt-logs/{branch}-remove.log`
- Use `--no-background` for foreground (blocking)

## EXAMPLES

Remove current worktree and branch:
```
wt remove
```

Remove specific worktree and branch:
```
wt remove feature-branch
```

Remove worktree but keep branch:
```
wt remove --no-delete-branch feature-branch
```

Remove multiple worktrees:
```
wt remove old-feature another-branch
```

Remove in foreground (blocking):
```
wt remove --no-background feature-branch
```

Switch to default in main:
```
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
    #[command(after_long_help = r#"## OPERATION

Commit → Squash → Rebase → Pre-merge hooks → Push → Cleanup → Post-merge hooks

**Commit**: Uncommitted changes are staged and committed with LLM-generated message.
Use `--tracked-only` to stage only tracked files.

**Squash**: Multiple commits are squashed into one with LLM-generated message.
Skip with `--no-squash`. Safety backup: `git reflog show refs/wt-backup/<branch>`

**Rebase**: Branch is rebased onto target. Conflicts abort the merge immediately.

**Hooks**: Pre-merge commands run after rebase (failures abort). Post-merge commands
run after cleanup (failures logged). Skip all with `--no-verify`.

**Push**: Fast-forward push to local target branch. Non-fast-forward pushes are rejected.

**Cleanup**: Worktree and branch are removed. Skip with `--no-remove`.

## EXAMPLES

Basic merge to main:
```
wt merge
```

Merge without squashing:
```
wt merge --no-squash
```

Keep worktree after merging:
```
wt merge --no-remove
```

Skip all hooks:
```
wt merge --no-verify
```"#)]
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
