use clap::Subcommand;

use super::SwitchFormat;
use crate::commands::Shell;

#[derive(Subcommand)]
pub enum ConfigShellCommand {
    /// Generate shell integration code
    #[command(
        after_long_help = r#"Outputs shell code for `eval` or sourcing. Most users should run `wt config shell install` instead, which adds this automatically.

## Manual setup

Add one line to the shell config:

Bash (~/.bashrc):
```console
$ eval "$(wt config shell init bash)"
```

Fish (~/.config/fish/config.fish):
```fish
wt config shell init fish | source
```

Zsh (~/.zshrc):
```zsh
eval "$(wt config shell init zsh)"
```

Nushell [experimental] — save to vendor autoload directory:
```console
$ wt config shell init nu | save -f ($nu.default-config-dir | path join vendor/autoload/wt.nu)
```"#
    )]
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
    #[command(
        after_long_help = r#"Detects existing shell config files and adds the integration line.

## Examples

Install for all detected shells:
```console
$ wt config shell install
```

Install for specific shell only:
```console
$ wt config shell install zsh
```

Shows proposed changes and waits for confirmation before modifying any files.
Use --yes to skip confirmation."#
    )]
    Install {
        /// Shell to install (default: all)
        #[arg(value_enum)]
        shell: Option<Shell>,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,

        /// Show what would be changed
        #[arg(long)]
        dry_run: bool,

        /// Command name for shell integration (defaults to binary name)
        ///
        /// Use this to create shell integration for an alternate command name.
        /// For example, `--cmd=git-wt` creates a `git-wt` shell function
        /// instead of `wt`, useful on Windows where `wt` conflicts with Windows Terminal.
        #[arg(long)]
        cmd: Option<String>,
    },

    /// Remove shell integration from config files
    #[command(
        after_long_help = r#"Removes shell integration lines from config files.

## Examples

Uninstall from all shells:
```console
$ wt config shell uninstall
```

Uninstall from specific shell only:
```console
$ wt config shell uninstall zsh
```

Skip confirmation prompt:
```console
$ wt config shell uninstall --yes
```

## Version tolerance

Detects various forms of the integration pattern regardless of:
- Command prefix (wt, worktree, etc.)
- Minor syntax variations between versions"#
    )]
    Uninstall {
        /// Shell to uninstall (default: all)
        #[arg(value_enum)]
        shell: Option<Shell>,

        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,

        /// Show what would be changed
        #[arg(long)]
        dry_run: bool,
    },

    /// Show output theme samples
    #[command(
        after_long_help = r#"Displays samples of all output message types to preview how worktrunk output will appear in the terminal.

## Message types

- Progress, success, error, warning, hint, info
- Gutter formatting for quoted content
- Prompts for user input
- Color palette showing each color rendered in itself"#
    )]
    ShowTheme,

    /// Generate static shell completions for package managers
    ///
    /// Outputs static completion scripts for Homebrew and other package managers.
    /// Only completes commands and flags, not branch names.
    /// This is predominantly for package managers. Users should run
    /// `wt config shell install` instead.
    #[command(hide = true)]
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Subcommand)]
pub enum ConfigPluginsOpencodeCommand {
    /// Install the activity tracking plugin
    #[command(
        after_long_help = r#"Writes the worktrunk plugin to the OpenCode plugins directory.

## Examples

```console
$ wt config plugins opencode install
$ wt config plugins opencode install --yes
```

## Plugin location

The plugin is written to `~/.config/opencode/plugins/worktrunk.ts`.
Override with the `OPENCODE_CONFIG_DIR` environment variable."#
    )]
    Install {
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// Remove the activity tracking plugin
    #[command(
        after_long_help = r#"Removes the worktrunk plugin from the OpenCode plugins directory.

## Examples

```console
$ wt config plugins opencode uninstall
```"#
    )]
    Uninstall {
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum ApprovalsCommand {
    /// Store approvals in approvals.toml
    #[command(
        after_long_help = r#"Prompts for approval of all project commands and saves them to approvals.toml.

By default, shows only unapproved commands. Use `--all` to review all commands
including previously approved ones."#
    )]
    Add {
        /// Show all commands
        #[arg(long)]
        all: bool,
    },

    /// Clear approved commands from approvals.toml
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
pub enum ConfigPluginsCommand {
    /// Claude Code plugin
    #[command(after_long_help = r#"## Examples

Install the plugin:
```console
$ wt config plugins claude install
```

Remove the plugin:
```console
$ wt config plugins claude uninstall
```

Configure the statusline:
```console
$ wt config plugins claude install-statusline
```"#)]
    Claude {
        #[command(subcommand)]
        action: ConfigPluginsClaudeCommand,
    },

    /// OpenCode plugin
    #[command(
        after_long_help = r#"Activity tracking plugin — shows status markers in `wt list`:
- 🤖 — agent is working
- 💬 — agent is waiting for input

## Examples

```console
$ wt config plugins opencode install
$ wt config plugins opencode uninstall
```

## Plugin location

Written to `~/.config/opencode/plugins/worktrunk.ts` (or `$OPENCODE_CONFIG_DIR/plugins/worktrunk.ts`)."#
    )]
    Opencode {
        #[command(subcommand)]
        action: ConfigPluginsOpencodeCommand,
    },
}

#[derive(Subcommand)]
pub enum ConfigPluginsClaudeCommand {
    /// Install the Worktrunk plugin
    #[command(
        after_long_help = r#"Adds the Worktrunk plugin from the marketplace and installs it. Equivalent to:

```console
$ claude plugin marketplace add max-sixty/worktrunk
$ claude plugin install worktrunk@worktrunk
```

Requires `claude` CLI. Skips gracefully if already installed."#
    )]
    Install {
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// Remove the Worktrunk plugin
    #[command(
        after_long_help = r#"Uninstalls the Worktrunk plugin from Claude Code. Equivalent to:

```console
$ claude plugin uninstall worktrunk@worktrunk
```"#
    )]
    Uninstall {
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// Configure the Claude Code statusline
    #[command(
        name = "install-statusline",
        after_long_help = r#"Writes the statusline configuration to `~/.claude/settings.json`, setting:

```json
{"statusLine":{"type":"command","command":"wt list statusline --format=claude-code"}}
```

Preserves existing settings. Creates the `.claude/` directory and `settings.json` if needed.

Skips gracefully if the statusline is already configured."#
    )]
    InstallStatusline {
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
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
            include_str!("../../dev/config.example.toml"),
            "```\n\n",
            "## Project config\n\n",
            "With `--project`, creates `.config/wt.toml` in the current repository:\n\n```\n",
            include_str!("../../dev/wt.example.toml"),
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
and project config (`.config/wt.toml`). Also shows system config if present.

If a config file doesn't exist, shows defaults that would be used.

## Full diagnostics

Use `--full` to run diagnostic checks:

```console
$ wt config show --full
```

This tests:
- **CI tool status** — Whether `gh` (GitHub) or `glab` (GitLab) is installed and authenticated
- **Commit generation** — Whether the LLM command can generate commit messages
- **Version check** — Whether a newer version is available on GitHub"#
    )]
    Show {
        /// Run diagnostic checks (CI tools, commit generation, version)
        #[arg(long)]
        full: bool,

        /// Output format (text, json)
        #[arg(long, default_value = "text", help_heading = "Output")]
        format: SwitchFormat,
    },

    /// Update deprecated config settings
    #[command(
        after_long_help = r#"Updates deprecated settings in user and project config files
to their current equivalents. Shows a diff and asks for confirmation.

## Examples

Preview and apply updates:
```console
$ wt config update
```

Apply without confirmation:
```console
$ wt config update --yes
```"#
    )]
    Update {
        /// Skip confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// Plugin management
    #[command(
        after_long_help = r#"Install and manage Worktrunk plugins for AI coding tools.

## Supported tools

- **claude** — Claude Code plugin (activity tracking + statusline)
- **opencode** — OpenCode plugin (activity tracking)

## Examples

```console
$ wt config plugins claude install
$ wt config plugins opencode install
```"#
    )]
    Plugins {
        #[command(subcommand)]
        action: ConfigPluginsCommand,
    },

    /// Manage internal data and cache
    #[command(
        after_long_help = r#"State is stored in `.git/` (config entries and log files), separate from configuration files.

## Keys

- **default-branch**: The repository's default branch (`main`, `master`, etc.)
- **previous-branch**: Previous branch for `wt switch -`
- **ci-status**: CI/PR status for a branch (passed, running, failed, conflicts, no-ci, error)
- **marker**: Custom status marker for a branch (shown in `wt list`)
- **vars**: [experimental] [Custom variables per branch](@/config.md#wt-config-state-vars)
- **logs**: Operation and debug logs

## Examples

Get the default branch:
```console
$ wt config state default-branch
```

Set the default branch manually:
```console
$ wt config state default-branch set main
```

Set a marker for current branch:
```console
$ wt config state marker set 🚧
```

Store arbitrary data:
```console
$ wt config state vars set env=staging
```

Clear all CI status cache:
```console
$ wt config state ci-status clear --all
```

Show all stored state:
```console
$ wt config state get
```

Clear all stored state:
```console
$ wt config state clear
```
<!-- subdoc: default-branch -->
<!-- subdoc: ci-status -->
<!-- subdoc: marker -->
<!-- subdoc: vars -->
<!-- subdoc: logs -->"#
    )]
    State {
        #[command(subcommand)]
        action: StateCommand,
    },
}

#[derive(Subcommand)]
pub enum StateCommand {
    /// Default branch detection and override
    #[command(
        name = "default-branch",
        after_long_help = r#"Useful in scripts to avoid hardcoding `main` or `master`:

```console
$ git rebase $(wt config state default-branch)
```

Without a subcommand, runs `get`. Use `set` to override, or `clear` then `get` to re-detect.

## Detection

Worktrunk detects the default branch automatically:

1. **Worktrunk cache** — Checks `git config worktrunk.default-branch` (single command)
2. **Git cache** — Detects primary remote and checks its HEAD (e.g., `origin/HEAD`)
3. **Remote query** — If not cached, queries `git ls-remote` (100ms–2s)
4. **Local inference** — If no remote, infers from local branches

Once detected, the result is cached in `worktrunk.default-branch` for fast access.

The local inference fallback uses these heuristics in order:
- If only one local branch exists, uses it
- For bare repos or empty repos, checks `symbolic-ref HEAD`
- Checks `git config init.defaultBranch`
- Looks for common names: `main`, `master`, `develop`, `trunk`"#
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

Without a subcommand, runs `get`. Use `set` to override or `clear` to reset."#
    )]
    PreviousBranch {
        #[command(subcommand)]
        action: Option<PreviousBranchAction>,
    },

    /// CI status cache
    #[command(
        name = "ci-status",
        after_long_help = r#"Caches GitHub/GitLab CI status for display in [`wt list`](@/list.md#ci-status).

Requires `gh` (GitHub) or `glab` (GitLab) CLI, authenticated. Platform auto-detects from remote URL; override with `forge.platform = "github"` in `.config/wt.toml` for SSH host aliases or self-hosted instances. For GitHub Enterprise or self-hosted GitLab, also set `forge.hostname`.

Checks open PRs/MRs first, then branch pipelines for branches with upstream. Local-only branches (no remote tracking) show blank.

Results cache for 30-60 seconds. Indicators dim when local changes haven't been pushed.

## Status values

| Status | Meaning |
|--------|---------|
| `passed` | All checks passed |
| `running` | Checks in progress |
| `failed` | Checks failed |
| `conflicts` | PR has merge conflicts |
| `no-ci` | No checks configured |
| `error` | Fetch error (rate limit, network, auth) |

See [`wt list` CI status](@/list.md#ci-status) for display symbols and colors.

Without a subcommand, runs `get` for the current branch. Use `clear` to reset cache for a branch or `clear --all` to reset all."#
    )]
    CiStatus {
        #[command(subcommand)]
        action: Option<CiStatusAction>,
    },

    /// Branch markers
    #[command(
        after_long_help = r#"Custom status text or emoji shown in the `wt list` Status column.

## Display

Markers appear at the end of the Status column, after git symbols:

<!-- wt list (markers) -->
```console
wt list
```

## Use cases

- **Work status** — `🚧` WIP, `✅` ready for review, `🔥` urgent
- **Agent tracking** — The [Claude Code](@/claude-code.md) plugin sets markers automatically
- **Notes** — Any short text: `"blocked"`, `"needs tests"`

## Storage

Stored in git config as `worktrunk.state.<branch>.marker`. Set directly with:

```console
$ git config worktrunk.state.feature.marker '{"marker":"🚧","set_at":0}'
```

Without a subcommand, runs `get` for the current branch. For `--branch`, use `get --branch=NAME`."#
    )]
    Marker {
        #[command(subcommand)]
        action: Option<MarkerAction>,
    },

    /// Operation and debug logs
    #[command(
        after_long_help = r#"View and manage log files — hook output, command audit trail, and debug diagnostics.

## What's logged

Three kinds of logs live in `.git/wt/logs/`:

### Command log (`commands.jsonl`)

All hook executions and LLM commands are recorded automatically — one JSON object per line. Rotates to `commands.jsonl.old` at 1MB (~2MB total). Fields:

| Field | Description |
|-------|-------------|
| `ts` | ISO 8601 timestamp |
| `wt` | The `wt` command that triggered this (e.g., `wt hook pre-merge --yes`) |
| `label` | What ran (e.g., `pre-merge user:lint`, `commit.generation`) |
| `cmd` | Shell command executed |
| `exit` | Exit code (`null` for background commands) |
| `dur_ms` | Duration in milliseconds (`null` for background commands) |

The command log appends entries and is not branch-specific — it records all activity across all worktrees.

### Hook output logs

| Operation | Log file |
|-----------|----------|
| Background hooks | `{branch}-{hash}-{source}-{hook-type}-{name}-{hash}.log` |
| Background removal | `{branch}-{hash}-remove.log` |

All `post-*` hooks (post-start, post-switch, post-commit, post-merge) run in the background and produce log files. Source is `user` or `project`. Hash suffixes are added by filename sanitization. Same operation on same branch overwrites the previous log. Logs from deleted branches remain until manually cleared.

### Diagnostic files

| File | Created when |
|------|-------------|
| `verbose.log` | Running with `-vv` |
| `diagnostic.md` | Running with `-vv` when warnings occur |

`verbose.log` is overwritten on each `-vv` run. `diagnostic.md` is a markdown report for pasting into GitHub issues — written only when warnings occur.

## Location

All logs are stored in `.git/wt/logs/` (in the main worktree's git directory). All worktrees write to the same directory.

## Examples

List all log files:
```console
$ wt config state logs get
```

Query the command log:
```console
$ tail -5 .git/wt/logs/commands.jsonl | jq .
```

View a specific hook log:
```console
$ cat "$(git rev-parse --git-dir)/wt/logs/feature-a1b-project-post-start-build-seq.log"
```

Clear all logs:
```console
$ wt config state logs clear
```"#
    )]
    Logs {
        #[command(subcommand)]
        action: Option<LogsAction>,
    },

    /// One-time hints shown in this repo
    #[command(
        after_long_help = r#"Some hints show once per repo on first use, then are recorded in git config
as `worktrunk.hints.<name> = true`.

## Current hints

| Name | Trigger | Message |
|------|---------|---------|
| `worktree-path` | First `wt switch --create` | Customize worktree locations: wt config create |

## Examples

```console
$ wt config state hints              # list shown hints
$ wt config state hints clear        # re-show all hints
$ wt config state hints clear NAME   # re-show specific hint
```"#
    )]
    Hints {
        #[command(subcommand)]
        action: Option<HintsAction>,
    },

    /// \[experimental\] Custom variables per branch
    #[command(
        name = "vars",
        after_long_help = r#"Store custom variables per branch. Values are stored as-is — plain strings or JSON.

## Examples

Set and get values:
```console
$ wt config state vars set env=staging
$ wt config state vars get env
```

Store JSON:
```console
$ wt config state vars set config='{"port": 3000, "debug": true}'
```

List all keys:
```console
$ wt config state vars list
```

Operate on a different branch:
```console
$ wt config state vars set env=production --branch=main
```

## Template access

Variables are available in [hook templates](@/hook.md#template-variables) as `{{ vars.<key> }}`. Use the `default` filter for keys that may not be set:

```toml
[post-start]
dev = "ENV={{ vars.env | default('development') }} npm start -- --port {{ vars.port | default('3000') }}"
```

JSON object and array values support dot access:

```console
$ wt config state vars set config='{"port": 3000, "debug": true}'
```
```toml
[post-start]
dev = "npm start -- --port {{ vars.config.port }}"
```

## Storage format

Stored in git config as `worktrunk.state.<branch>.vars.<key>`. Keys must contain only letters, digits, hyphens, and underscores — dots conflict with git config's section separator."#
    )]
    Vars {
        #[command(subcommand)]
        action: VarsAction,
    },

    /// Get all stored state
    #[command(after_long_help = r#"Shows all stored state including:

- **Default branch**: Cached result of querying remote for default branch
- **Previous branch**: Previous branch for `wt switch -`
- **Branch markers**: User-defined branch notes
- **Vars**: Custom variables per branch
- **CI status**: Cached GitHub/GitLab CI status per branch (30s TTL)
- **Hints**: One-time hints that have been shown
- **Log files**: Operation and debug logs

CI cache entries show status, age, and the commit SHA they were fetched for."#)]
    Get {
        /// Output format (table, json)
        #[arg(long, value_enum, default_value = "table", hide_possible_values = true)]
        format: super::OutputFormat,
    },

    /// Clear all stored state
    #[command(after_long_help = r#"Clears all stored state:

- Default branch cache
- Previous branch
- All branch markers
- All variables
- All CI status cache
- All hints
- All log files
- Stale trash from worktree removal (`.git/wt/trash/`)

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
$ wt config state default-branch
```

Clear cache and re-detect:
```console
$ wt config state default-branch clear && wt config state default-branch get
```"#)]
    Get,

    /// Set the default branch
    #[command(after_long_help = r#"## Examples

Set the default branch:
```console
$ wt config state default-branch set main
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
$ wt config state previous-branch
```"#)]
    Get,

    /// Set the previous branch
    #[command(after_long_help = r#"## Examples

Set the previous branch:
```console
$ wt config state previous-branch set feature
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
        after_long_help = r#"Returns: passed, running, failed, conflicts, no-ci, or error.

## Examples

Get CI status for current branch:
```console
$ wt config state ci-status
```

Get CI status for a specific branch:
```console
$ wt config state ci-status get --branch=feature
```

Clear cache and re-fetch:
```console
$ wt config state ci-status clear && wt config state ci-status get
```"#
    )]
    Get {
        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,

        /// Output format (text, json)
        #[arg(long, default_value = "text", help_heading = "Output")]
        format: SwitchFormat,
    },

    /// Clear CI status cache
    #[command(after_long_help = r#"## Examples

Clear CI status for current branch:
```console
$ wt config state ci-status clear
```

Clear CI status for a specific branch:
```console
$ wt config state ci-status clear --branch=feature
```

Clear all CI status cache:
```console
$ wt config state ci-status clear --all
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
$ wt config state marker
```

Get marker for a specific branch:
```console
$ wt config state marker get --branch=feature
```"#)]
    Get {
        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,

        /// Output format (text, json)
        #[arg(long, default_value = "text", help_heading = "Output")]
        format: SwitchFormat,
    },

    /// Set marker for a branch
    #[command(after_long_help = r#"## Examples

Set marker for current branch:
```console
$ wt config state marker set 🚧
```

Set marker for a specific branch:
```console
$ wt config state marker set "✅ ready" --branch=feature
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
$ wt config state marker clear
```

Clear marker for a specific branch:
```console
$ wt config state marker clear --branch=feature
```

Clear all markers:
```console
$ wt config state marker clear --all
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
    /// Get log file paths
    #[command(
        after_long_help = r#"Lists log files, or gets the path to a specific log.

## Examples

List all log files:
```console
$ wt config state logs
```

Get path to a specific hook log:
```console
$ wt config state logs get --hook=user:post-start:server
```

Stream a hook's log output:
```console
$ tail -f "$(wt config state logs get --hook=user:post-start:server)"
```

Get log for background worktree removal:
```console
$ wt config state logs get --hook=internal:remove
```

Get log for a different branch:
```console
$ wt config state logs get --hook=user:post-start:server --branch=feature
```"#
    )]
    Get {
        /// Get path for a specific log file
        ///
        /// Format: source:hook-type:name (e.g., user:post-start:server) for
        /// hook commands, or internal:op (e.g., internal:remove) for internal
        /// operations.
        #[arg(long)]
        hook: Option<String>,

        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,

        /// Output format (text, json)
        #[arg(
            long,
            default_value = "text",
            conflicts_with = "hook",
            help_heading = "Output"
        )]
        format: SwitchFormat,
    },

    /// Clear all log files
    Clear,
}

#[derive(Subcommand)]
pub enum HintsAction {
    /// List hints that have been shown
    #[command(
        after_long_help = r#"Lists which one-time hints have been shown in this repository.

## Examples

List shown hints:
```console
$ wt config state hints
```"#
    )]
    Get {
        /// Output format (text, json)
        #[arg(long, default_value = "text", help_heading = "Output")]
        format: SwitchFormat,
    },

    /// Clear hints (re-show on next trigger)
    #[command(
        after_long_help = r#"Clears hint state so hints will show again on next trigger.

## Examples

Clear all hints:
```console
$ wt config state hints clear
```

Clear a specific hint:
```console
$ wt config state hints clear worktree-path
```"#
    )]
    Clear {
        /// Specific hint to clear (clears all if not specified)
        name: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum VarsAction {
    /// Get a value
    #[command(after_long_help = r#"## Examples

Get a value for the current branch:
```console
$ wt config state vars get env
```

Get a value for a specific branch:
```console
$ wt config state vars get env --branch=feature
```"#)]
    Get {
        /// Key name
        key: String,

        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,
    },

    /// Set a value
    #[command(after_long_help = r#"## Examples

Set a plain string:
```console
$ wt config state vars set env=staging
```

Set JSON:
```console
$ wt config state vars set config='{"port": 3000}'
```

Set for a specific branch:
```console
$ wt config state vars set env=production --branch=main
```"#)]
    Set {
        /// KEY=VALUE pair
        #[arg(value_name = "KEY=VALUE", value_parser = super::parse_vars_assignment)]
        assignment: (String, String),

        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,
    },

    /// List all keys
    #[command(after_long_help = r#"## Examples

List keys for current branch:
```console
$ wt config state vars list
```

List keys for a specific branch:
```console
$ wt config state vars list --branch=feature
```"#)]
    List {
        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,

        /// Output format (text, json)
        #[arg(long, default_value = "text", help_heading = "Output")]
        format: SwitchFormat,
    },

    /// Clear a key or all keys
    #[command(after_long_help = r#"## Examples

Clear a specific key:
```console
$ wt config state vars clear env
```

Clear all keys for current branch:
```console
$ wt config state vars clear --all
```

Clear all keys for a specific branch:
```console
$ wt config state vars clear env --branch=feature
```"#)]
    Clear {
        /// Key to clear (required unless --all)
        #[arg(conflicts_with = "all")]
        key: Option<String>,

        /// Clear all keys for the branch
        #[arg(long)]
        all: bool,

        /// Target branch (defaults to current)
        #[arg(long, add = crate::completion::branch_value_completer())]
        branch: Option<String>,
    },
}
