use clap::builder::styling::{AnsiColor, Color, Styles};
use clap::{Command, CommandFactory, Parser, Subcommand, ValueEnum};
use std::sync::OnceLock;

use crate::commands::Shell;

/// Shell type for directive output in --internal mode
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum DirectiveShell {
    /// POSIX shells (bash, zsh, fish) - outputs `cd '/path'`
    #[default]
    Posix,
    /// PowerShell - outputs `Set-Location '/path'`
    Powershell,
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
#[command(after_long_help = "\
Getting started

  wt switch --create feature    Create worktree and branch
  wt switch feature             Switch to existing worktree
  wt merge                      Squash, rebase, and merge to main

Run `wt config shell` to set up directory switching.

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

    /// Show commands and debug info
    #[arg(
        long,
        short = 'v',
        global = true,
        display_order = 102,
        help_heading = "Global Options"
    )]
    pub verbose: bool,

    /// Shell wrapper mode (optionally specify shell type for directive output)
    /// Usage: --internal (defaults to posix) or --internal=powershell
    #[arg(long, global = true, hide = true, default_missing_value = "posix", num_args = 0..=1, require_equals = true)]
    pub internal: Option<DirectiveShell>,

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

        /// Command name for shell integration (defaults to binary name)
        ///
        /// Use this to create shell integration for an alternate command name.
        /// For example, `--cmd=git-wt` creates a `git-wt` shell function
        /// instead of `wt`, useful on Windows where `wt` conflicts with Windows Terminal.
        #[arg(long)]
        cmd: Option<String>,
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

        /// Command name for shell integration (defaults to binary name)
        ///
        /// Use this to create shell integration for an alternate command name.
        /// For example, `--cmd=git-wt` creates a `git-wt` shell function
        /// instead of `wt`, useful on Windows where `wt` conflicts with Windows Terminal.
        #[arg(long)]
        cmd: Option<String>,
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

    /// Show output theme samples
    #[command(after_long_help = r#"## Output Theme

Displays samples of all output message types:
- Progress, success, error, warning, hint, info
- Gutter formatting for quoted content
- Prompts for user input

Use this to preview how worktrunk output will appear in the terminal."#)]
    ShowTheme,
}

#[derive(Subcommand)]
pub enum ApprovalsCommand {
    /// Store approvals in config
    #[command(
        after_long_help = r#"Prompts for approval of all project commands and saves them to user config.

By default, shows only unapproved commands. Use `--all` to review all commands
including previously approved ones."#
    )]
    Add {
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

    /// Create configuration file
    #[command(
        after_long_help = concat!(
            "## User config\n\n",
            "Creates `~/.config/worktrunk/config.toml` with the following content:\n\n```\n",
            include_str!("../dev/config.example.toml"),
            "```\n\n",
            "## Project config\n\n",
            "With `--project`, creates `.config/wt.toml` in the current repository:\n\n```\n",
            include_str!("../dev/wt.example.toml"),
            "```"
        )
    )]
    Create {
        /// Create project config (`.config/wt.toml`) instead of user config
        #[arg(long)]
        project: bool,
    },

    /// Show configuration files & locations
    #[command(
        after_long_help = r#"Shows location and contents of user config (`~/.config/worktrunk/config.toml`)
and project config (`.config/wt.toml`).

If a config file doesn't exist, shows defaults that would be used.

## Full diagnostics

Use `--full` to test commit generation with a synthetic diff:

```console
wt config show --full
```

This verifies that the LLM command is configured correctly and can generate
commit messages."#
    )]
    Show {
        /// Test commit generation pipeline
        #[arg(long)]
        full: bool,
    },

    /// Get, set, or clear stored state
    #[command(
        after_long_help = r#"State is stored in `.git/` (config entries and log files), separate from configuration files.
Use `wt config show` to view file-based configuration.

## Keys

- **default-branch**: The repository's default branch (main, master, etc.)
- **previous-branch**: Previous branch for `wt switch -`
- **ci-status**: CI/PR status for a branch (passed, running, failed, conflicts, noci)
- **marker**: Custom status marker for a branch (shown in `wt list`)
- **logs**: Background operation logs

## Examples

Get the default branch:
```console
wt config state default-branch
```

Set the default branch manually:
```console
wt config state default-branch set main
```

Set a marker for current branch:
```console
wt config state marker set "🚧 WIP"
```

Clear all CI status cache:
```console
wt config state ci-status clear --all
```

Show all stored state:
```console
wt config state get
```

Clear all stored state:
```console
wt config state clear
```

<!-- subdoc: default-branch -->

<!-- subdoc: ci-status -->

<!-- subdoc: marker -->

<!-- subdoc: logs -->"#
    )]
    State {
        #[command(subcommand)]
        action: StateCommand,
    },
}

#[derive(Subcommand)]
pub enum StateCommand {
    /// Default branch setting
    #[command(
        name = "default-branch",
        after_long_help = r#"## Detection

Worktrunk detects the default branch automatically:

1. **Local cache** — Checks `git rev-parse origin/HEAD` (fast, no network)
2. **Remote query** — If not cached, queries `git ls-remote` (100ms–2s)
3. **Cache result** — Stores via `git remote set-head` for future calls
4. **Local inference** — If no remote, infers from local branches

The local inference fallback uses these heuristics in order:
- If only one local branch exists, uses it
- Checks what HEAD points to in main worktree
- Checks `git config init.defaultBranch`
- Looks for common names: main, master, develop

## When to use

Most users never need this command — default branch detection is automatic.
Use it to:

- **Debug** — See what worktrunk thinks the default branch is
- **Override** — Set a non-standard default branch with `set`
- **Refresh** — Force re-query with `get --refresh` after remote changes
- **Clear** — Remove cached value with `clear`

Without a subcommand, runs `get`. For `--refresh`, use `get --refresh`."#
    )]
    DefaultBranch {
        #[command(subcommand)]
        action: Option<DefaultBranchAction>,
    },

    /// Previous branch (for `wt switch -`)
    #[command(
        name = "previous-branch",
        after_long_help = r#"Enables `wt switch -` to return to the previous worktree, similar to `cd -` or `git checkout -`.

## How it works

Updated automatically on every `wt switch`. Stored in git config as `worktrunk.history`.

## When to use

Most users never need this command — the previous branch updates automatically.
Use it to:

- **Debug** — See what `-` resolves to
- **Override** — Set a specific branch as "previous"
- **Clear** — Reset the history

Without a subcommand, runs `get`."#
    )]
    PreviousBranch {
        #[command(subcommand)]
        action: Option<PreviousBranchAction>,
    },

    /// CI status cache
    #[command(
        name = "ci-status",
        after_long_help = r#"Caches GitHub/GitLab CI status for display in [wt list](@/list.md#ci-status).

## How it works

1. **Platform detection** — Detected from remote URL (github.com → GitHub, gitlab.com → GitLab)
2. **CLI requirement** — Requires `gh` (GitHub) or `glab` (GitLab) CLI, authenticated
3. **What's checked** — PRs/MRs first, then branch pipelines for branches with upstream
4. **Caching** — Results cached 30-60 seconds per branch+commit

## Status values

| Status | Meaning |
|--------|---------|
| `passed` | All checks passed |
| `running` | Checks in progress |
| `failed` | Checks failed |
| `conflicts` | PR has merge conflicts |
| `noci` | No checks configured |

See [wt list CI status](@/list.md#ci-status) for display symbols and colors.

## When to use

- **Debug** — See cached status and when it was fetched
- **Refresh** — Force re-fetch with `get --refresh`
- **Clear** — Remove stale cache entries

Without a subcommand, runs `get` for the current branch. For `--branch` or `--refresh`, use `get --branch=NAME`."#
    )]
    CiStatus {
        #[command(subcommand)]
        action: Option<CiStatusAction>,
    },

    /// Branch markers
    #[command(
        after_long_help = r#"Custom status text or emoji shown in the `wt list` Status column.

## Display

Markers appear at the start of the Status column:

```
Branch    Status   Path
main      ^        ~/code/myproject
feature   🚧↑      ~/code/myproject.feature
bugfix    🤖!↑⇡    ~/code/myproject.bugfix
```

## Use cases

- **Work status** — `🚧` WIP, `✅` ready for review, `🔥` urgent
- **Agent tracking** — The [Claude Code plugin](@/claude-code.md) sets markers automatically
- **Notes** — Any short text: `"blocked"`, `"needs tests"`

## Storage

Stored in git config as `worktrunk.marker.<branch>`. Set directly with:

```console
git config worktrunk.marker.feature "🚧"
```

Without a subcommand, runs `get` for the current branch. For `--branch`, use `get --branch=NAME`."#
    )]
    Marker {
        #[command(subcommand)]
        action: Option<MarkerAction>,
    },

    /// Background operation logs
    #[command(after_long_help = r#"View and manage logs from background operations.

## What's logged

| Operation | Log file |
|-----------|----------|
| post-start hooks | `{branch}-{source}-post-start-{name}.log` |
| Background removal | `{branch}-remove.log` |

Source is `user` or `project` depending on where the hook is defined.

## Location

All logs are stored in `.git/wt-logs/` (in the main worktree's git directory).

## Behavior

- **Overwrites** — Same operation on same branch overwrites previous log
- **Persists** — Logs from deleted branches remain until manually cleared
- **Shared** — All worktrees write to the same log directory

## Examples

List all log files:
```console
wt config state logs get
```

View a specific log:
```console
cat "$(git rev-parse --git-dir)/wt-logs/feature-project-post-start-build.log"
```

Clear all logs:
```console
wt config state logs clear
```"#)]
    Logs {
        #[command(subcommand)]
        action: Option<LogsAction>,
    },

    /// Get all stored state
    #[command(after_long_help = r#"Shows all stored state including:

- **Default branch**: Cached result of querying remote for default branch
- **Previous branch**: Previous branch for `wt switch -`
- **Branch markers**: User-defined branch notes
- **CI status**: Cached GitHub/GitLab CI status per branch (30s TTL)
- **Log files**: Background operation logs

CI cache entries show status, age, and the commit SHA they were fetched for."#)]
    Get {
        /// Output format (table, json)
        #[arg(long, value_enum, default_value = "table", hide_possible_values = true)]
        format: OutputFormat,
    },

    /// Clear all stored state
    #[command(after_long_help = r#"Clears all stored state:

- Default branch cache
- Previous branch
- All branch markers
- All CI status cache
- All log files

Use individual subcommands (`default-branch clear`, `ci-status clear --all`, etc.)
to clear specific state."#)]
    Clear,
}

#[derive(Subcommand)]
pub enum DefaultBranchAction {
    /// Get the default branch
    #[command(after_long_help = r#"## Examples

Get the default branch:
```console
wt config state default-branch
```

Force refresh from remote:
```console
wt config state default-branch get --refresh
```"#)]
    Get {
        /// Force refresh from remote
        #[arg(long)]
        refresh: bool,
    },

    /// Set the default branch
    #[command(after_long_help = r#"## Examples

Set the default branch:
```console
wt config state default-branch set main
```"#)]
    Set {
        /// Branch name to set as default
        #[arg(add = crate::completion::branch_value_completer())]
        branch: String,
    },

    /// Clear the default branch cache
    Clear,
}

#[derive(Subcommand)]
pub enum PreviousBranchAction {
    /// Get the previous branch
    #[command(after_long_help = r#"## Examples

Get the previous branch (used by `wt switch -`):
```console
wt config state previous-branch
```"#)]
    Get,

    /// Set the previous branch
    #[command(after_long_help = r#"## Examples

Set the previous branch:
```console
wt config state previous-branch set feature
```"#)]
    Set {
        /// Branch name to set as previous
        #[arg(add = crate::completion::branch_value_completer())]
        branch: String,
    },

    /// Clear the previous branch
    Clear,
}

#[derive(Subcommand)]
pub enum CiStatusAction {
    /// Get CI status for a branch
    #[command(
        after_long_help = r#"Returns: passed, running, failed, conflicts, noci, or error.

## Examples

Get CI status for current branch:
```console
wt config state ci-status
```

Force refresh from GitHub/GitLab:
```console
wt config state ci-status get --refresh
```

Get CI status for a specific branch:
```console
wt config state ci-status get --branch=feature
```"#
    )]
    Get {
        /// Force refresh from GitHub/GitLab
        #[arg(long)]
        refresh: bool,

        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,
    },

    /// Clear CI status cache
    #[command(after_long_help = r#"## Examples

Clear CI status for current branch:
```console
wt config state ci-status clear
```

Clear CI status for a specific branch:
```console
wt config state ci-status clear --branch=feature
```

Clear all CI status cache:
```console
wt config state ci-status clear --all
```"#)]
    Clear {
        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer(), conflicts_with = "all")]
        branch: Option<String>,

        /// Clear all CI status cache
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
pub enum MarkerAction {
    /// Get marker for a branch
    #[command(after_long_help = r#"## Examples

Get marker for current branch:
```console
wt config state marker
```

Get marker for a specific branch:
```console
wt config state marker get --branch=feature
```"#)]
    Get {
        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,
    },

    /// Set marker for a branch
    #[command(after_long_help = r#"## Examples

Set marker for current branch:
```console
wt config state marker set "🚧 WIP"
```

Set marker for a specific branch:
```console
wt config state marker set "✅ ready" --branch=feature
```"#)]
    Set {
        /// Marker text (shown in `wt list` output)
        value: String,

        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,
    },

    /// Clear marker for a branch
    #[command(after_long_help = r#"## Examples

Clear marker for current branch:
```console
wt config state marker clear
```

Clear marker for a specific branch:
```console
wt config state marker clear --branch=feature
```

Clear all markers:
```console
wt config state marker clear --all
```"#)]
    Clear {
        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer(), conflicts_with = "all")]
        branch: Option<String>,

        /// Clear all markers
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
pub enum LogsAction {
    /// List background operation log files
    #[command(
        after_long_help = r#"Lists log files from background operations like post-start commands and worktree removal.

## Examples

List log files:
```console
wt config state logs
```"#
    )]
    Get,

    /// Clear background operation logs
    Clear,
}

/// Run individual workflow operations
#[derive(Subcommand)]
pub enum StepCommand {
    /// Commit changes with LLM commit message
    ///
    /// Stages working tree changes and commits with an LLM-generated message.
    Commit {
        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,

        /// Skip hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,

        /// What to stage before committing [default: all]
        #[arg(long)]
        stage: Option<crate::commands::commit::StageMode>,

        /// Show prompt without running LLM
        ///
        /// Outputs the rendered prompt to stdout for debugging or manual piping.
        #[arg(long)]
        show_prompt: bool,
    },

    /// Squash commits down to target
    ///
    /// Stages working tree changes, combines all commits since the target into a single commit, generates message with LLM.
    Squash {
        /// Target branch
        ///
        /// Defaults to default branch.
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,

        /// Skip hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,

        /// What to stage before committing [default: all]
        #[arg(long)]
        stage: Option<crate::commands::commit::StageMode>,

        /// Show prompt without running LLM
        ///
        /// Outputs the rendered prompt to stdout for debugging or manual piping.
        #[arg(long)]
        show_prompt: bool,
    },

    /// Push changes to local target branch
    ///
    /// Fast-forwards the target branch to include current commits.
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

    /// \[experimental\] Run command in each worktree
    #[command(
        after_long_help = r#"Executes a command sequentially in every worktree with real-time output. Continues on failure and shows a summary at the end.

Context JSON is piped to stdin for scripts that need structured data.

## Template variables

All variables are shell-escaped:

| Variable | Description |
|----------|-------------|
| `{{ branch }}` | Branch name (slashes replaced with dashes) |
| `{{ worktree }}` | Absolute path to the worktree |
| `{{ worktree_name }}` | Worktree directory name |
| `{{ repo }}` | Repository name |
| `{{ repo_root }}` | Absolute path to the main repository root |
| `{{ commit }}` | Current HEAD commit SHA (full) |
| `{{ short_commit }}` | Current HEAD commit SHA (7 chars) |
| `{{ default_branch }}` | Default branch name (e.g., "main") |
| `{{ remote }}` | Primary remote name (e.g., "origin") |
| `{{ remote_url }}` | Primary remote URL |
| `{{ upstream }}` | Upstream tracking branch, if configured |

## Examples

Check status across all worktrees:

```console
wt step for-each -- git status --short
```

Run npm install in all worktrees:

```console
wt step for-each -- npm install
```

Use branch name in command:

```console
wt step for-each -- "echo Branch: {{ branch }}"
```

Pull updates in worktrees with upstreams (skips others):

```console
git fetch --prune && wt step for-each -- '[ "$(git rev-parse @{u} 2>/dev/null)" ] || exit 0; git pull --autostash'
```

Note: This command is experimental and may change in future versions.
"#
    )]
    ForEach {
        /// Command template (see --help for all variables)
        #[arg(required = true, last = true, num_args = 1..)]
        args: Vec<String>,
    },
}

/// Run hooks independently
#[derive(Subcommand)]
pub enum HookCommand {
    /// Show configured hooks
    ///
    /// Lists user and project hooks. Project hooks show approval status (❓ = needs approval).
    Show {
        /// Hook type to show (default: all)
        #[arg(value_parser = ["post-create", "post-start", "pre-commit", "pre-merge", "post-merge", "pre-remove"])]
        hook_type: Option<String>,

        /// Show expanded commands with current variables
        #[arg(long)]
        expanded: bool,
    },

    /// Run post-create hooks
    ///
    /// Blocking — waits for completion before continuing.
    PostCreate {
        /// Run only this command from hook config
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,
    },

    /// Run post-start hooks
    ///
    /// Background — runs without blocking.
    PostStart {
        /// Run only this command from hook config
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,
    },

    /// Run pre-commit hooks
    PreCommit {
        /// Run only this command from hook config
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,
    },

    /// Run pre-merge hooks
    PreMerge {
        /// Run only this command from hook config
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,
    },

    /// Run post-merge hooks
    PostMerge {
        /// Run only this command from hook config
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,
    },

    /// Run pre-remove hooks
    PreRemove {
        /// Run only this command from hook config
        #[arg(add = crate::completion::hook_command_name_completer())]
        name: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        force: bool,
    },

    /// Manage command approvals
    #[command(after_long_help = r#"## How Approvals Work

Commands from project hooks (`.config/wt.toml`) and LLM configuration require
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
wt hook approvals add
```

Clear approvals for current project:
```console
wt hook approvals clear
```

Clear global approvals:
```console
wt hook approvals clear --global
```"#)]
    Approvals {
        #[command(subcommand)]
        action: ApprovalsCommand,
    },
}

/// Subcommands for `wt list`
#[derive(Subcommand)]
pub enum ListSubcommand {
    /// Single-line status for shell prompts
    ///
    /// For shell prompts, starship, or editor integrations.
    #[command(after_long_help = "Format: `branch  status  ±working  commits  upstream  ci`")]
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
approved-commands = [
    "post-create.install = npm ci",
    "pre-merge.test = npm test",
]
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

See [wt hook](@/hook.md#user-hooks) for complete documentation.

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

See [wt hook](@/hook.md) for complete documentation on hook types, execution order, template variables, and [JSON context](@/hook.md#json-context).

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

<!-- subdoc: create -->

<!-- subdoc: state -->
"#
    )]
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },

    /// Run individual workflow operations
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
- `push` — Push to target branch (default: main)
- `for-each` — [experimental] Run a command in every worktree

## See also

- [wt merge](@/merge.md) — Runs commit → squash → rebase → hooks → push → cleanup automatically
- [wt hook](@/hook.md) — Run hooks independently

<!-- subdoc: for-each -->
"#
    )]
    Step {
        #[command(subcommand)]
        action: StepCommand,
    },

    /// Run hooks independently
    #[command(
        name = "hook",
        after_long_help = r#"Run hooks independently of normal worktree operations.

Hooks normally run automatically during `wt switch --create`, `wt merge`, and `wt remove`. This command runs them on demand — useful for testing hooks during development, running in CI pipelines, or re-running after a failure.

Both user hooks (from `~/.config/worktrunk/config.toml`) and project hooks (from `.config/wt.toml`) are supported.

```console
wt hook pre-merge           # Run pre-merge hooks
wt hook pre-merge --force   # Skip approval prompts (for CI)
```

## Hook types

| Hook | When | Blocking | Fail-fast |
|------|------|----------|-----------|
| `post-create` | After worktree created | Yes | No |
| `post-start` | After worktree created | No (background) | No |
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

Runs after successful merge in the **main worktree**, **best-effort**. Failures are logged but don't abort.

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

See [wt merge](@/merge.md#pipeline) for the complete pipeline.

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
| `{{ repo }}` | my-project | Repository name |
| `{{ branch }}` | feature-foo | Branch name |
| `{{ worktree }}` | /path/to/worktree | Absolute worktree path |
| `{{ worktree_name }}` | my-project.feature-foo | Worktree directory name |
| `{{ repo_root }}` | /path/to/main | Repository root path |
| `{{ default_branch }}` | main | Default branch name |
| `{{ commit }}` | a1b2c3d4e5f6... | Full HEAD commit SHA |
| `{{ short_commit }}` | a1b2c3d | Short HEAD commit SHA |
| `{{ remote }}` | origin | Primary remote name |
| `{{ remote_url }}` | git@github.com:user/repo.git | Remote URL |
| `{{ upstream }}` | origin/feature | Upstream tracking branch |
| `{{ target }}` | main | Target branch (merge hooks only) |

### JSON context

Hooks also receive context as JSON on stdin, enabling hooks in any language:

```python
import json, sys
ctx = json.load(sys.stdin)
print(f"Setting up {ctx['repo']} on branch {ctx['branch']}")
```

The JSON includes all template variables plus `hook_type` and `hook_name`.

## Security

Project commands require approval on first run:

```
🟡 repo needs approval to execute 3 commands:

⚪ post-create install:
   echo 'Installing dependencies...'

❓ Allow and remember? [y/N]
```

- Approvals are saved to user config (`~/.config/worktrunk/config.toml`)
- If a command changes, new approval is required
- Use `--force` to bypass prompts (useful for CI/automation)
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

Skip hooks with `--no-verify`.

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

## Examples

### Node.js / TypeScript

```toml
[post-create]
install = "npm ci"

[post-start]
dev = "npm run dev"

[pre-commit]
lint = "npm run lint"
typecheck = "npm run typecheck"

[pre-merge]
test = "npm test"
build = "npm run build"
```

### Rust

```toml
[post-create]
build = "cargo build"

[pre-commit]
format = "cargo fmt -- --check"
clippy = "cargo clippy -- -D warnings"

[pre-merge]
test = "cargo test"
build = "cargo build --release"

[post-merge]
install = "cargo install --path ."
```

### Python (uv)

```toml
[post-create]
install = "uv sync"

[pre-commit]
format = "uv run ruff format --check ."
lint = "uv run ruff check ."

[pre-merge]
test = "uv run pytest"
typecheck = "uv run mypy ."
```

### Monorepo

```toml
[post-create]
frontend = "cd frontend && npm ci"
backend = "cd backend && cargo build"

[post-start]
database = "docker-compose up -d postgres"

[pre-merge]
frontend-tests = "cd frontend && npm test"
backend-tests = "cd backend && cargo test"
```

### Common patterns

**Fast dependencies + slow build** — Install blocking, build in background:

```toml
post-create = "npm install"
post-start = "npm run build"
```

**Progressive validation** — Quick checks before commit, thorough validation before merge:

```toml
[pre-commit]
lint = "npm run lint"
typecheck = "npm run typecheck"

[pre-merge]
test = "npm test"
build = "npm run build"
```

**Target-specific behavior**:

```toml
post-merge = """
if [ "{{ target }}" = "main" ]; then
    npm run deploy:production
elif [ "{{ target }}" = "staging" ]; then
    npm run deploy:staging
fi
"""
```

**Symlinks and caches** — The `{{ repo_root }}` variable points to the main worktree:

```toml
[post-create]
cache = "ln -sf {{ repo_root }}/node_modules node_modules"
env = "cp {{ repo_root }}/.env.local .env"
```

## See also

- [wt merge](@/merge.md) — Runs hooks automatically during merge
- [wt switch](@/switch.md) — Runs post-create/post-start hooks on `--create`
- [wt config](@/config.md) — Manage hook approvals

<!-- subdoc: approvals -->
"#
    )]
    Hook {
        #[command(subcommand)]
        action: HookCommand,
    },

    /// Interactive worktree selector
    ///
    /// Toggle preview tabs with 1/2/3 keys. Toggle preview visibility with alt-p.
    #[cfg(unix)]
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
2. **log** — Recent commits; commits already on main have dimmed hashes
3. **main…±** — Diff of all changes vs main branch

## Keybindings

| Key | Action |
|-----|--------|
| `↑`/`↓` | Navigate worktree list |
| `Enter` | Switch to selected worktree |
| `Esc` | Cancel |
| (type) | Filter worktrees |
| `1`/`2`/`3` | Switch preview tab |
| `Alt-p` | Toggle preview panel |
| `Ctrl-u`/`Ctrl-d` | Scroll preview up/down |

Branches without worktrees are included — selecting one creates a worktree. (`wt list` requires `--branches` to show them.)

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
| main↕ | Commits ahead/behind main |
| main…± | Line diffs in commits ahead of main (`--full`) |
| Path | Worktree directory |
| Remote⇅ | Commits ahead/behind tracking branch |
| CI | Pipeline status (`--full`) |
| Commit | Short hash (8 chars) |
| Age | Time since last commit |
| Message | Last commit message (truncated) |

### CI status

The CI column shows GitHub/GitLab pipeline status:

| Indicator | Meaning |
|-----------|---------|
| `●` green | All checks passed |
| `●` blue | Checks running |
| `●` red | Checks failed |
| `●` yellow | Merge conflicts with base |
| `●` gray | No checks configured |
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
| | `⚑` | Path doesn't match template |
| | `⊟` | Prunable (directory missing) |
| | `⊞` | Locked worktree |
| Main | `^` | Is the main branch |
| | `✗` | Would conflict if merged to main |
| | `_` | Same commit as main, clean |
| | `–` | Same commit as main, uncommitted changes |
| | `⊂` | [Content integrated](@/remove.md#branch-cleanup) |
| | `↕` | Diverged from main |
| | `↑` | Ahead of main |
| | `↓` | Behind main |
| Remote | `\|` | In sync with remote |
| | `⇅` | Diverged from remote |
| | `⇡` | Ahead of remote |
| | `⇣` | Behind remote |

Rows are dimmed when [safe to delete](@/remove.md#branch-cleanup) (`_` same commit with clean working tree or `⊂` content integrated).

## JSON output

Query structured data with `--format=json`:

```console
# Worktrees with merge conflicts
wt list --format=json | jq '.[] | select(.operation_state == "conflicts")'

# Uncommitted changes
wt list --format=json | jq '.[] | select(.working_tree.modified)'

# Current worktree
wt list --format=json | jq '.[] | select(.is_current)'

# Branches ahead of main
wt list --format=json | jq '.[] | select(.main.ahead > 0)'

# Integrated branches (ready to clean up)
wt list --format=json | jq '.[] | select(.main_state == "integrated" or .main_state == "empty")'
```

**Fields:**

| Field | Description |
|-------|-------------|
| `branch` | Branch name (null for detached HEAD) |
| `path` | Worktree path (absent for branches without worktrees) |
| `kind` | `"worktree"` or `"branch"` |
| `commit` | `{sha, short_sha, message, timestamp}` |
| `working_tree` | `{staged, modified, untracked, renamed, deleted, diff, diff_vs_main}` |
| `main_state` | `"is_main"` `"would_conflict"` `"empty"` `"same_commit"` `"integrated"` `"diverged"` `"ahead"` `"behind"` |
| `integration_reason` | `"ancestor"` `"trees_match"` `"no_added_changes"` `"merge_adds_nothing"` (when `main_state == "integrated"`) |
| `operation_state` | `"conflicts"` `"rebase"` `"merge"` (absent when no operation in progress) |
| `main` | `{ahead, behind, diff}` (absent when `is_main`) |
| `remote` | `{name, branch, ahead, behind}` (absent when no tracking branch) |
| `worktree` | `{state, reason, detached, bare}` |
| `is_main` | Main worktree |
| `is_current` | Current worktree |
| `is_previous` | Previous worktree from [wt switch](@/switch.md) |
| `pr` | `{ci, source, stale, url}` — CI status from PR or branch (absent when no CI) |
| `statusline` | Pre-formatted status with ANSI colors |
| `symbols` | Raw status symbols without colors (e.g., `"!?↓"`) |

## See also

- [wt select](@/select.md) — Interactive worktree picker with live preview
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

        /// Show CI and `main` diffstat
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
- **Create new worktree** (`--create`) — Creates branch and worktree, runs [hooks](@/hook.md)

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
3. Runs [post-create hooks](@/hook.md#post-create) (blocking)
4. Switches to new directory
5. Spawns [post-start hooks](@/hook.md#post-start) (background)

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

        /// Skip hooks
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

A branch is safe to delete when its content is already reflected in the target. Worktrunk checks four conditions (in order of cost):

1. **Same commit** — Branch HEAD is literally the same commit as target.
2. **No added changes** — Three-dot diff (`main...branch`) shows no files. The branch has no file changes beyond the merge-base (includes "branch is ancestor" case).
3. **Tree contents match** — Branch tree SHA equals main tree SHA. Commit history differs but file contents are identical (e.g., after a revert or merge commit pulling in main).
4. **Merge adds nothing** — Simulated merge (`git merge-tree`) produces the same tree as main. Handles squash-merged branches where main has since advanced.

In `wt list`, `_` indicates same commit, and `⊂` indicates content is integrated. Branches showing either are dimmed as safe to delete.

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
        #[arg(add = crate::completion::local_branches_completer())]
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

        /// Skip hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,

        /// Skip approval prompts
        #[arg(long)]
        force: bool,
    },

    /// Merge worktree into target branch
    ///
    /// Squashes commits, rebases, runs hooks, merges to target, and removes the worktree.
    #[command(
        after_long_help = r#"When already on the target branch or in the main worktree, the worktree is preserved automatically.

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
3. **Pre-merge hooks** — Project commands run after rebase, before merge. Failures abort. See [wt hook](@/hook.md).
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

        /// Force running hooks
        #[arg(long, overrides_with = "no_verify", hide = true)]
        verify: bool,

        /// Skip hooks
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
