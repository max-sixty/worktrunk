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
    apply_help_template_recursive(Cli::command(), DEFAULT_COMMAND_NAME)
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
#[command(arg_required_else_help = true)]
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

    /// Shell wrapper mode
    #[arg(long, global = true, hide = true)]
    pub internal: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum ConfigShellCommand {
    /// Generate shell integration code
    #[command(after_long_help = r#"## Manual Setup

Add one line to the shell config:

Bash (~/.bashrc):
```console
eval "$(wt config shell init bash)"
```

Fish (~/.config/fish/config.fish):
```fish
wt config shell init fish | source
```

Zsh (~/.zshrc):
```zsh
eval "$(wt config shell init zsh)"
```

## Auto setup

Use `wt config shell install` to add to the shell config automatically."#)]
    Init {
        /// Shell to generate code for
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Write shell integration to config files
    #[command(after_long_help = r#"## Auto setup

Detects existing shell config files and adds integration:
```console
wt config shell install
```

Install for specific shell only:
```console
wt config shell install zsh
```

Shows proposed changes and waits for confirmation before modifying any files.
Use --force to skip confirmation."#)]
    Install {
        /// Shell to install (default: all)
        #[arg(value_enum)]
        shell: Option<Shell>,

        /// Skip confirmation prompt
        #[arg(short, long)]
        force: bool,
    },

    /// Remove shell integration from config files
    #[command(after_long_help = r#"## Removal

Removes shell integration lines from config files:
```console
wt config shell uninstall
```

Remove from specific shell only:
```console
wt config shell uninstall zsh
```

Skip confirmation prompt:
```console
wt config shell uninstall --force
```

## Version tolerance

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
    Add {
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

    /// Create user configuration file
    #[command(
        after_long_help = concat!(
            "Creates `~/.config/worktrunk/config.toml` with the following content:\n\n```\n",
            include_str!("../dev/config.example.toml"),
            "```"
        )
    )]
    Create,

    /// Show configuration files & locations
    #[command(
        after_long_help = r#"Shows location and contents of user config (`~/.config/worktrunk/config.toml`)
and project config (`.config/wt.toml`).

If a config file doesn't exist, shows defaults that would be used.

## Doctor mode

Use `--doctor` to test commit generation with a synthetic diff:

```console
wt config show --doctor
```

This verifies that the LLM command is configured correctly and can generate
commit messages."#
    )]
    Show {
        /// Test commit generation pipeline
        #[arg(long)]
        doctor: bool,
    },

    /// Manage caches (CI status, default branch)
    Cache {
        #[command(subcommand)]
        action: CacheCommand,
    },

    /// Get or set runtime variables (stored in git config)
    #[command(
        after_long_help = r#"Variables are runtime values stored in git config, separate from
configuration files. Use `wt config show` to view file-based configuration.

## Available variables

- **default-branch**: The repository's default branch (read-only, cached)
- **marker**: Custom status marker for a branch (shown in `wt list`)

## Examples

Get the default branch:
```console
wt config var get default-branch
```

Set a marker for current branch:
```console
wt config var set marker "🚧 WIP"
```

Clear markers:
```console
wt config var clear marker --all
```"#
    )]
    Var {
        #[command(subcommand)]
        action: VarCommand,
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
```console
wt config approvals add
```

Clear approvals for current project:
```console
wt config approvals clear
```

Clear global approvals:
```console
wt config approvals clear --global
```"#)]
    Approvals {
        #[command(subcommand)]
        action: ApprovalsCommand,
    },
}

#[derive(Subcommand)]
pub enum CacheCommand {
    /// Show cached data
    #[command(after_long_help = r#"Shows all cached data including:

- **Default branch**: Cached result of querying remote for default branch
- **CI status**: Cached GitHub/GitLab CI status per branch (30s TTL)

CI cache entries show status, age, and the commit SHA they were fetched for."#)]
    Show,

    /// Clear cached data
    Clear {
        /// Cache type: 'ci', 'default-branch', or 'logs' (default: all)
        #[arg(value_parser = ["ci", "default-branch", "logs"])]
        cache_type: Option<String>,
    },

    /// Refresh default branch from remote
    #[command(
        after_long_help = r#"Queries the remote to determine the default branch and caches the result.

Use when the remote default branch has changed. The cached value is used by
`wt merge`, `wt list`, and other commands that reference the default branch."#
    )]
    Refresh,
}

#[derive(Subcommand)]
pub enum VarCommand {
    /// Get a variable value
    #[command(after_long_help = r#"Variables:

- **default-branch**: The repository's default branch (main, master, etc.)
- **marker**: Custom status marker for a branch (shown in `wt list`)
- **ci-status**: CI/PR status for a branch (passed, running, failed, conflicts, noci)

## Examples

Get the default branch:
```console
wt config var get default-branch
```

Force refresh from remote:
```console
wt config var get default-branch --refresh
```

Get marker for current branch:
```console
wt config var get marker
```

Get marker for a specific branch:
```console
wt config var get marker --branch=feature
```

Get CI status for current branch:
```console
wt config var get ci-status
```

Force refresh CI status (bypass cache):
```console
wt config var get ci-status --refresh
```"#)]
    Get {
        /// Variable: 'default-branch', 'marker', or 'ci-status'
        #[arg(value_parser = ["default-branch", "marker", "ci-status"])]
        key: String,

        /// Force refresh (for cached variables)
        #[arg(long)]
        refresh: bool,

        /// Target branch (for branch-scoped variables)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,
    },

    /// Set a variable value
    #[command(after_long_help = r#"Variables:

- **marker**: Custom status marker displayed in `wt list` output

## Examples

Set marker for current branch:
```console
wt config var set marker "🚧 WIP"
```

Set marker for a specific branch:
```console
wt config var set marker "✅ ready" --branch=feature
```"#)]
    Set {
        /// Variable: 'marker'
        #[arg(value_parser = ["marker"])]
        key: String,

        /// Value to set
        value: String,

        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,
    },

    /// Clear a variable value
    #[command(after_long_help = r#"Variables:

- **marker**: Custom status marker for a branch

## Examples

Clear marker for current branch:
```console
wt config var clear marker
```

Clear marker for a specific branch:
```console
wt config var clear marker --branch=feature
```

Clear all markers:
```console
wt config var clear marker --all
```"#)]
    Clear {
        /// Variable: 'marker'
        #[arg(value_parser = ["marker"])]
        key: String,

        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer(), conflicts_with = "all")]
        branch: Option<String>,

        /// Clear all values
        #[arg(long)]
        all: bool,
    },
}

/// Run individual workflow operations
#[derive(Subcommand)]
pub enum StepCommand {
    /// Commit changes with LLM commit message
    ///
    /// Stages working tree changes based on `--stage` and commits them.
    /// Generates the commit message using an LLM. Runs pre-commit hooks
    /// unless `--no-verify` is passed.
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

    /// Squash commits down to target
    ///
    /// Combines all commits since diverging from the target branch into a single
    /// commit. Stages and includes working tree changes based on `--stage`.
    /// Generates the commit message using an LLM. Runs pre-commit hooks
    /// unless `--no-verify` is passed.
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

    /// Run pre-remove hook
    PreRemove {
        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,
    },
}

/// Subcommands for `wt list`
#[derive(Subcommand)]
pub enum ListSubcommand {
    /// Single-line status for shell prompts
    ///
    /// Format: `branch  status  ±working  commits  upstream  ci`
    ///
    /// Designed for shell prompts, starship, or editor integrations.
    /// Uses same collection infrastructure as `wt list`.
    Statusline {
        /// Claude Code mode: read context from stdin, add directory and model
        ///
        /// Reads JSON from stdin with `.workspace.current_dir` and `.model.display_name`.
        /// Output: `dir  branch  status  ±working  commits  upstream  ci  | model`
        #[arg(long)]
        claude_code: bool,
    },
}

#[derive(Subcommand)]
pub enum Commands {
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

Show current configuration and file locations:

```console
wt config show
```

## User config

The user config stores personal preferences that apply across all repositories. Create it with `wt config create` and view with `wt config show`.

### Worktree path template

Controls where new worktrees are created. The template is relative to the repository root.

**Available variables:**
- `{{ main_worktree }}` — main worktree directory name
- `{{ branch }}` — branch name (slashes replaced with dashes)

**Examples** for a repo at `~/code/myproject` creating branch `feature/login`:

```toml
# Default — siblings in parent directory
# Creates: ~/code/myproject.feature-login
worktree-path = "../{{ main_worktree }}.{{ branch }}"

# Inside the repository
# Creates: ~/code/myproject/.worktrees/feature-login
worktree-path = ".worktrees/{{ branch }}"

# Namespaced (useful when multiple repos share a parent directory)
# Creates: ~/code/worktrees/myproject/feature-login
worktree-path = "../worktrees/{{ main_worktree }}/{{ branch }}"
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
commit = false  # Skip committing uncommitted changes (--no-commit)
remove = false  # Keep worktree after merge (--no-remove)
verify = false  # Skip project hooks (--no-verify)
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
approved-commands = [
    "post-create.install = npm ci",
    "pre-merge.test = npm test",
]
```

Manage approvals with `wt config approvals list` and `wt config approvals clear <repo>`.

## Project config

The project config defines lifecycle hooks — commands that run at specific points during worktree operations. This file is checked into version control and shared across the team.

Create `.config/wt.toml` in the repository root:

```toml
[post-create]
install = "npm ci"

[pre-merge]
test = "npm test"
lint = "npm run lint"
```

See [Hooks](@/hooks.md) for complete documentation on hook types, execution order, and template variables.

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

### Special variables

| Variable | Purpose |
|----------|---------|
| `WORKTRUNK_CONFIG_PATH` | Override user config file location (not a config key) |
| `NO_COLOR` | Disable colored output ([standard](https://no-color.org/)) |
| `CLICOLOR_FORCE` | Force colored output even when not a TTY |
"#
    )]
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },

    /// Run individual workflow operations
    #[command(
        name = "step",
        after_long_help = r#"Run individual workflow operations: commits, squashes, rebases, pushes, and [hooks](@/hooks.md).

## Examples

Commit with LLM-generated message:

```console
wt step commit
```

Run pre-merge hooks in CI:

```console
wt step pre-merge --force
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

**Git operations:**

- `commit` — Stage and commit with [LLM-generated message](@/llm-commits.md)
- `squash` — Squash all branch commits into one with [LLM-generated message](@/llm-commits.md)
- `rebase` — Rebase onto target branch
- `push` — Push to target branch (default: main)

**Hooks** — run project commands defined in [`.config/wt.toml`](@/hooks.md):

- `post-create` — After worktree creation (blocking)
- `post-start` — After worktree creation (background)
- `pre-commit` — Before committing
- `pre-merge` — Before pushing to target
- `post-merge` — After merge cleanup

## See also

- [wt merge](@/merge.md) — Runs commit → squash → rebase → hooks → push → cleanup automatically
"#
    )]
    Step {
        #[command(subcommand)]
        action: StepCommand,
    },

    /// Interactive worktree selector
    ///
    /// Toggle preview tabs with 1/2/3 keys. Toggle preview visibility with alt-p.
    #[cfg(unix)]
    #[command(
        after_long_help = r#"Interactive worktree picker with live preview. Navigate worktrees with keyboard shortcuts and press Enter to switch.

<!-- demo: wt-select.gif -->

## Examples

Open the selector:

```console
wt select
```

## Preview tabs

Toggle between views with number keys:

1. **HEAD±** — Diff of uncommitted changes
2. **history** — Recent commits on the branch
3. **main…±** — Diff of all changes vs main branch

## Keybindings

| Key | Action |
|-----|--------|
| `↑`/`↓` or `j`/`k` | Navigate worktree list |
| `Enter` | Switch to selected worktree |
| `Esc` or `q` | Cancel |
| `/` | Filter worktrees |
| `1`/`2`/`3` | Switch preview tab |
| `Alt+p` | Toggle preview panel |
| `Ctrl-u`/`Ctrl-d` | Scroll preview up/down |

## See also

- [wt list](@/list.md) — Static table view with all worktree metadata
- [wt switch](@/switch.md) — Direct switching when you know the target branch
"#
    )]
    Select,

    /// List worktrees and optionally branches
    #[command(
        after_long_help = r#"Show all worktrees with their status. The table includes uncommitted changes, divergence from main and remote, and optional CI status.

The table renders progressively: branch names, paths, and commit hashes appear immediately, then status, divergence, and other columns fill in as background git operations complete. With `--full`, CI status fetches from the network — the table displays instantly and CI fills in as results arrive.

## Examples

List all worktrees:

<!-- wt list -->
```console
$ wt list
```

Include CI status and conflict detection:

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
| main↕ | Commits ahead/behind main |
| main…± | Line diffs in commits ahead of main (`--full`) |
| Path | Worktree directory |
| Remote⇅ | Commits ahead/behind tracking branch |
| CI | Pipeline status (`--full`) |
| Commit | Short hash (8 chars) |
| Age | Time since last commit |
| Message | Last commit message (truncated) |

The CI column shows GitHub/GitLab pipeline status:

| Indicator | Meaning |
|-----------|---------|
| `●` green | All checks passed |
| `●` blue | Checks running |
| `●` red | Checks failed |
| `●` yellow | Merge conflicts with base |
| `●` gray | No checks configured |
| (blank) | No PR/MR found |

Any CI dot appears dimmed when there are unpushed local changes (stale status). CI indicators are clickable links to the PR page.

## Status symbols

Symbols appear in the Status column in this order:

| Category | Symbol | Meaning |
|----------|--------|---------|
| Working tree | `+` | Staged files |
| | `!` | Modified files (unstaged) |
| | `?` | Untracked files |
| | `✖` | Merge conflicts |
| | `↻` | Rebase in progress |
| | `⋈` | Merge in progress |
| Branch state | `⊘` | Would conflict if merged to main (`--full` only) |
| | `≡` | Matches main (identical contents) |
| | `_` | No commits ahead, clean working tree |
| Main divergence | `^` | Is the main branch |
| | `↑` | Ahead of main |
| | `↓` | Behind main |
| | `↕` | Diverged from main |
| Remote divergence | `\|` | In sync with remote |
| | `⇡` | Ahead of remote |
| | `⇣` | Behind remote |
| | `⇅` | Diverged from remote |
| Worktree state | `/` | Branch without worktree |
| | `⚑` | Path doesn't match template |
| | `⌫` | Prunable (directory missing) |
| | `⊠` | Locked worktree |

Rows are dimmed when the branch [content is already in main](@/remove.md#branch-cleanup) (`≡` matches main or `_` no commits ahead).

## JSON output

Query structured data with `--format=json`:

```console
# Worktrees with conflicts
wt list --format=json | jq '.[] | select(.status.branch_state == "Conflicts")'

# Uncommitted changes
wt list --format=json | jq '.[] | select(.status.working_tree.modified)'

# Current worktree
wt list --format=json | jq '.[] | select(.is_current == true)'

# Branches ahead of main
wt list --format=json | jq '.[] | select(.status.main_divergence == "Ahead")'
```

**Status fields:**
- `working_tree`: `{untracked, modified, staged, renamed, deleted}`
- `branch_state`: `""` | `"Conflicts"` | `"MergeTreeConflicts"` | `"MatchesMain"` | `"NoCommits"`
- `git_operation`: `""` | `"Rebase"` | `"Merge"`
- `main_divergence`: `""` | `"Ahead"` | `"Behind"` | `"Diverged"`
- `upstream_divergence`: `""` | `"Ahead"` | `"Behind"` | `"Diverged"`

**Position fields:**
- `is_main` — Main worktree
- `is_current` — Current directory
- `is_previous` — Previous worktree from [wt switch](@/switch.md)

## See also

- [wt select](@/select.md) — Interactive worktree picker with live preview
"#
    )]
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
    #[command(after_long_help = r#"Two distinct operations:

- **Switch to existing worktree** — Changes directory, nothing else
- **Create new worktree** (`--create`) — Creates branch and worktree, runs [hooks](@/hooks.md)

## Examples

```console
wt switch feature-auth           # Switch to existing worktree
wt switch -                      # Previous worktree (like cd -)
wt switch --create new-feature   # Create branch and worktree
wt switch --create hotfix --base production
```

For interactive selection, use [`wt select`](@/select.md).

## Creating worktrees

With `--create`, worktrunk:

1. Creates branch from `--base` (defaults to default branch)
2. Creates worktree at configured path
3. Runs [post-create hooks](@/hooks.md#post-create) (blocking)
4. Switches to new directory
5. Spawns [post-start hooks](@/hooks.md#post-start) (background)

```console
wt switch --create api-refactor
wt switch --create fix --base release-2.0
wt switch --create docs --execute "code ."
wt switch --create temp --no-verify      # Skip hooks
```

## Shortcuts

| Shortcut | Meaning |
|----------|---------|
| `^` | Default branch (main/master) |
| `@` | Current branch/worktree |
| `-` | Previous worktree (like `cd -`) |

```console
wt switch -                      # Back to previous
wt switch ^                      # Main worktree
wt switch --create fix --base=@  # Branch from current HEAD
```

## See also

- [wt select](@/select.md) — Interactive worktree selection
- [wt list](@/list.md) — View all worktrees
- [wt remove](@/remove.md) — Delete worktrees when done
- [wt merge](@/merge.md) — Integrate changes back to main
"#)]
    Switch {
        /// Branch or worktree name
        ///
        /// Shortcuts: '^' (main), '-' (previous), '@' (current)
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
        /// Especially useful in shell aliases to create a worktree and start
        /// working in one command:
        ///
        /// ```sh
        /// alias wsc='wt switch --create --execute=claude'
        /// ```
        ///
        /// Then `wsc feature-branch` creates the worktree and launches Claude Code.
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
    #[command(
        after_long_help = r#"Removes worktrees and their branches. Without arguments, removes the current worktree and returns to the main worktree.

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

Branches delete automatically when their content is already in the target branch (typically main). This works with squash-merge and rebase workflows where commit history differs but file changes match.

A branch is safe to delete when its content is already reflected in the target. Worktrunk checks three conditions:

1. **No added changes** — Three-dot diff (`main...branch`) shows no files. The branch has no file changes beyond the merge-base.
2. **Tree contents match** — Branch tree SHA equals main tree SHA. Commit history differs but file contents are identical (e.g., after a revert or merge commit pulling in main).
3. **Merge adds nothing** — Simulated merge (`git merge-tree`) produces the same tree as main. Handles squash-merged branches where main has since advanced.

In `wt list`, `_` indicates no commits ahead of main, and `≡` indicates tree contents match. Branches showing either are dimmed as safe to delete.

Use `-D` to force-delete branches with unmerged changes. Use `--no-delete-branch` to keep the branch regardless of status.

## Background removal

Removal runs in the background by default (returns immediately). Logs are written to `.git/wt-logs/{branch}-remove.log`. Use `--no-background` to run in the foreground.

Arguments resolve by path first, then branch name. [Shortcuts](@/switch.md#shortcuts): `@` (current), `-` (previous), `^` (main worktree).

## See also

- [wt merge](@/merge.md) — Remove worktree after merging
- [wt list](@/list.md) — View all worktrees
"#
    )]
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

        /// Skip pre-remove hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,
    },

    /// Merge worktree into target branch
    #[command(
        after_long_help = r#"Merge the current branch into the target branch and clean up. Handles the full workflow: commit uncommitted changes, squash commits, rebase, run hooks, push to target, and remove the worktree.

When already on the target branch or in the main worktree, the worktree is preserved automatically.

## Examples

Basic merge to main:

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

Skip git operations, only run hooks and push:

```console
wt merge --no-commit
```

## Pipeline

`wt merge` runs these steps:

1. **Squash** — Stages uncommitted changes, then combines all commits since target into one (like GitHub's "Squash and merge"). Use `--stage` to control what gets staged: `all` (default), `tracked`, or `none`. A backup ref is saved to `refs/wt-backup/<branch>`. With `--no-squash`, uncommitted changes are committed separately and individual commits are preserved.
2. **Rebase** — Rebases onto target if behind. Skipped if already up-to-date. Conflicts abort immediately.
3. **Pre-merge hooks** — Project commands run after rebase, before merge. Failures abort. See [Hooks](@/hooks.md).
4. **Merge** — Fast-forward merge to the target branch. Non-fast-forward merges are rejected.
5. **Pre-remove hooks** — Project commands run before removing worktree. Failures abort.
6. **Cleanup** — Removes the worktree and branch. Use `--no-remove` to keep the worktree.
7. **Post-merge hooks** — Project commands run after cleanup. Failures are logged but don't abort.

Use `--no-commit` to skip all git operations (steps 1-2) and only run hooks and merge. Useful after preparing commits manually with `wt step`. Requires a clean working tree.

## See also

- [wt step](@/step.md) — Run individual merge steps (commit, squash, rebase, push)
- [wt remove](@/remove.md) — Remove worktrees without merging
- [wt switch](@/switch.md) — Navigate to other worktrees
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
