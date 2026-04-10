+++
title = "wt config"
description = "Manage user & project configs. Includes shell integration, hooks, and saved state."
weight = 15

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt config --help-page` — edit cli.rs to update -->

Manage user & project configs. Includes shell integration, hooks, and saved state.

## Examples

Install shell integration (required for directory switching):

{{ terminal(cmd="wt config shell install") }}

Create user config file with documented examples:

{{ terminal(cmd="wt config create") }}

Create project config file (`.config/wt.toml`) for hooks:

{{ terminal(cmd="wt config create --project") }}

Show current configuration and file locations:

{{ terminal(cmd="wt config show") }}

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
timeout-ms = 500   # Wall-clock budget (ms) for picker data collection; 0 disables
```

### Step

```toml
[step.copy-ignored]
exclude = []   # Additional excludes (e.g., [".cache/", ".turbo/"])
```

Built-in excludes always apply: VCS metadata directories (`.bzr/`, `.hg/`, `.jj/`, `.pijul/`, `.sl/`, `.svn/`) and tool-state directories (`.conductor/`, `.entire/`, `.pi/`, `.worktrees/`). User config and project config exclusions are combined.

### Aliases

Command templates that run with `wt step <name>`. See [`wt step` aliases](@/step.md#aliases) for usage and flags.

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

Single command:

```toml
pre-start = "npm ci"
```

Multiple named commands — concurrent for post-*, sequential for pre-*:

```toml
[pre-merge]
test = "npm test"
build = "npm run build"
```

Pipeline — list of maps, run in order (each map concurrent):

```toml
post-start = [
    { install = "npm ci" },
    { build = "npm run build", server = "npm run dev" }
]
```
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

Command templates that run with `wt step <name>`. See [`wt step` aliases](@/step.md#aliases) for usage and flags.

```toml
[aliases]
deploy = "make deploy BRANCH={{ branch }}"
url = "echo http://localhost:{{ branch | hash_port }}"
```

Aliases defined here are shared with teammates. For personal aliases, use the [user config](@/config.md#aliases) `[aliases]` section instead.
<!-- PROJECT_CONFIG_END -->

# Shell Integration

Worktrunk needs shell integration to change directories when switching worktrees. Install with:

{{ terminal(cmd="wt config shell install") }}

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

{{ terminal(cmd="WORKTRUNK_COMMIT__GENERATION__COMMAND=__WT_QUOT__echo 'test: automated commit'__WT_QUOT__ wt merge") }}

### Other environment variables

| Variable | Purpose |
|----------|---------|
| `WORKTRUNK_BIN` | Override binary path for shell wrappers; useful for testing dev builds |
| `WORKTRUNK_CONFIG_PATH` | Override user config file location |
| `WORKTRUNK_SYSTEM_CONFIG_PATH` | Override system config file location |
| `XDG_CONFIG_DIRS` | Colon-separated system config directories (default: `/etc/xdg`) |
| `WORKTRUNK_DIRECTIVE_FILE` | Internal: set by shell wrappers to enable directory changes |
| `WORKTRUNK_SHELL` | Internal: set by shell wrappers to indicate shell type (e.g., `powershell`) |
| `WORKTRUNK_MAX_CONCURRENT_COMMANDS` | Max parallel git commands (default: 32). Lower if hitting file descriptor limits. |
| `NO_COLOR` | Disable colored output ([standard](https://no-color.org/)) |
| `CLICOLOR_FORCE` | Force colored output even when not a TTY |

## Command reference

{% terminal() %}
wt config - Manage user &amp; project configs

Includes shell integration, hooks, and saved state.

Usage: <b><span class=c>wt config</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>shell</span></b>    Shell integration setup
  <b><span class=c>create</span></b>   Create configuration file
  <b><span class=c>show</span></b>     Show configuration files &amp; locations
  <b><span class=c>update</span></b>   Update deprecated config settings
  <b><span class=c>plugins</span></b>  Plugin management
  <b><span class=c>state</span></b>    Manage internal data and cache

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)
{% end %}

# Subcommands

## wt config show

Show configuration files & locations.

Shows location and contents of user config (`~/.config/worktrunk/config.toml`)
and project config (`.config/wt.toml`). Also shows system config if present.

If a config file doesn't exist, shows defaults that would be used.

### Full diagnostics

Use `--full` to run diagnostic checks:

{{ terminal(cmd="wt config show --full") }}

This tests:
- **CI tool status** — Whether `gh` (GitHub) or `glab` (GitLab) is installed and authenticated
- **Commit generation** — Whether the LLM command can generate commit messages
- **Version check** — Whether a newer version is available on GitHub

### Command reference

{% terminal() %}
wt config show - Show configuration files &amp; locations

Usage: <b><span class=c>wt config show</span></b> <span class=c>[OPTIONS]</span>

<b><span class=g>Options:</span></b>
      <b><span class=c>--full</span></b>
          Run diagnostic checks (CI tools, commit generation, version)

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Output:</span></b>
      <b><span class=c>--format</span></b><span class=c> &lt;FORMAT&gt;</span>
          Output format (text, json)

          Possible values:
          - <b><span class=c>text</span></b>: Human-readable text output
          - <b><span class=c>json</span></b>: JSON output

          [default: text]

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)
{% end %}

## wt config state

Manage internal data and cache.

State is stored in `.git/` (config entries and log files), separate from configuration files.

### Keys

- **default-branch**: [The repository's default branch (`main`, `master`, etc.)](@/config.md#wt-config-state-default-branch)
- **previous-branch**: Previous branch for `wt switch -`
- **logs**: [Operation and debug logs](@/config.md#wt-config-state-logs)
- **ci-status**: [CI/PR status for a branch (passed, running, failed, conflicts, no-ci, error)](@/config.md#wt-config-state-ci-status)
- **marker**: [Custom status marker for a branch (shown in `wt list`)](@/config.md#wt-config-state-marker)
- **vars**: <span class="badge-experimental"></span> [Custom variables per branch](@/config.md#wt-config-state-vars)

### Examples

Get the default branch:
{{ terminal(cmd="wt config state default-branch") }}

Set the default branch manually:
{{ terminal(cmd="wt config state default-branch set main") }}

Set a marker for current branch:
{{ terminal(cmd="wt config state marker set 🚧") }}

Store arbitrary data:
{{ terminal(cmd="wt config state vars set env=staging") }}

Clear all CI status cache:
{{ terminal(cmd="wt config state ci-status clear --all") }}

Show all stored state:
{{ terminal(cmd="wt config state get") }}

Clear all stored state:
{{ terminal(cmd="wt config state clear") }}

### Command reference

{% terminal() %}
wt config state - Manage internal data and cache

Usage: <b><span class=c>wt config state</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>get</span></b>              Get all stored state
  <b><span class=c>clear</span></b>            Clear all stored state
  <b><span class=c>default-branch</span></b>   Default branch detection and override
  <b><span class=c>previous-branch</span></b>  Previous branch (for <b>wt switch -</b>)
  <b><span class=c>logs</span></b>             Operation and debug logs
  <b><span class=c>hints</span></b>            One-time hints shown in this repo
  <b><span class=c>ci-status</span></b>        CI status cache
  <b><span class=c>marker</span></b>           Branch markers
  <b><span class=c>vars</span></b>             [experimental] Custom variables per branch

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)
{% end %}

## wt config state default-branch

Default branch detection and override.

Useful in scripts to avoid hardcoding `main` or `master`:

{{ terminal(cmd="git rebase $(wt config state default-branch)") }}

Without a subcommand, runs `get`. Use `set` to override, or `clear` then `get` to re-detect.

### Detection

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
- Looks for common names: `main`, `master`, `develop`, `trunk`

### Command reference

{% terminal() %}
wt config state default-branch - Default branch detection and override

Usage: <b><span class=c>wt config state default-branch</span></b> <span class=c>[OPTIONS]</span> <span class=c>[COMMAND]</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>get</span></b>    Get the default branch
  <b><span class=c>set</span></b>    Set the default branch
  <b><span class=c>clear</span></b>  Clear the default branch cache

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)
{% end %}

## wt config state logs

Operation and debug logs.

View and manage log files — hook output, command audit trail, and debug diagnostics.

### What's logged

Three kinds of logs live in `.git/wt/logs/`:

#### Command log (`commands.jsonl`)

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

#### Hook output logs

Hook output lives in per-branch subtrees under `.git/wt/logs/{branch}/`:

| Operation | Log path |
|-----------|----------|
| Background hooks | `{branch}/{source}/{hook-type}/{name}.log` |
| Background removal | `{branch}/internal/remove.log` |

All `post-*` hooks (post-start, post-switch, post-commit, post-merge) run in the background and produce log files. Source is `user` or `project`. Branch and hook names are sanitized for filesystem safety (invalid characters → `-`; short collision-avoidance hash appended). Same operation on same branch overwrites the previous log. Removing a branch clears its subtree; orphans from deleted branches can be swept with `wt config state logs clear`.

#### Diagnostic files

| File | Created when |
|------|-------------|
| `verbose.log` | Running with `-vv` |
| `diagnostic.md` | Running with `-vv` when warnings occur |

`verbose.log` is overwritten on each `-vv` run. `diagnostic.md` is a markdown report for pasting into GitHub issues — written only when warnings occur.

### Location

All logs are stored in `.git/wt/logs/` (in the main worktree's git directory). All worktrees write to the same directory. Top-level files are shared logs (command audit + diagnostics); top-level directories are per-branch log trees.

### Examples

List all log files:
{{ terminal(cmd="wt config state logs get") }}

Query the command log:
{{ terminal(cmd="tail -5 .git/wt/logs/commands.jsonl | jq .") }}

View a specific hook log:
{{ terminal(cmd="cat __WT_QUOT__$(git rev-parse --git-dir)/wt/logs/feature/project/post-start/build.log__WT_QUOT__") }}

Clear all logs:
{{ terminal(cmd="wt config state logs clear") }}

### Command reference

{% terminal() %}
wt config state logs - Operation and debug logs

Usage: <b><span class=c>wt config state logs</span></b> <span class=c>[OPTIONS]</span> <span class=c>[COMMAND]</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>get</span></b>    Get log file paths
  <b><span class=c>clear</span></b>  Clear all log files

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)
{% end %}

## wt config state ci-status

CI status cache.

Caches GitHub/GitLab CI status for display in [`wt list`](@/list.md#ci-status).

Requires `gh` (GitHub) or `glab` (GitLab) CLI, authenticated. Platform auto-detects from remote URL; override with `forge.platform = "github"` in `.config/wt.toml` for SSH host aliases or self-hosted instances. For GitHub Enterprise or self-hosted GitLab, also set `forge.hostname`.

Checks open PRs/MRs first, then branch pipelines for branches with upstream. Local-only branches (no remote tracking) show blank.

Results cache for 30-60 seconds. Indicators dim when local changes haven't been pushed.

### Status values

| Status | Meaning |
|--------|---------|
| `passed` | All checks passed |
| `running` | Checks in progress |
| `failed` | Checks failed |
| `conflicts` | PR has merge conflicts |
| `no-ci` | No checks configured |
| `error` | Fetch error (rate limit, network, auth) |

See [`wt list` CI status](@/list.md#ci-status) for display symbols and colors.

Without a subcommand, runs `get` for the current branch. Use `clear` to reset cache for a branch or `clear --all` to reset all.

### Command reference

{% terminal() %}
wt config state ci-status - CI status cache

Usage: <b><span class=c>wt config state ci-status</span></b> <span class=c>[OPTIONS]</span> <span class=c>[COMMAND]</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>get</span></b>    Get CI status for a branch
  <b><span class=c>clear</span></b>  Clear CI status cache

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)
{% end %}

## wt config state marker

Branch markers.

Custom status text or emoji shown in the `wt list` Status column.

### Display

Markers appear at the end of the Status column, after git symbols:

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__readme_example_list_marker.snap — edit source to update -->

{% terminal(cmd="wt list (markers)") %}
&#32;&#32;<b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>  <b>Remote⇅</b>  <b>Commit</b>    <b>Age</b>   <b>Message</b>
@ main             <span class=d>^</span><span class=d>⇡</span>                         <span class=g>⇡1</span>      <span class=d>33323bc1</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>
+ feature-api      <span class=d>↑</span> 🤖              <span class=g>↑1</span>               <span class=d>70343f03</span>  <span class=d>1d</span>    <span class=d>Add REST API endpoints</span>
+ review-ui      <span class=c>?</span> <span class=d>↑</span> 💬              <span class=g>↑1</span>               <span class=d>a585d6ed</span>  <span class=d>1d</span>    <span class=d>Add dashboard component</span>
+ wip-docs       <span class=c>?</span> <span class=d>–</span>                                  <span class=d>33323bc1</span>  <span class=d>1d</span>    <span class=d>Initial commit</span>

<span class=d>○</span> <span class=d>Showing 4 worktrees, 2 with changes, 2 ahead, 1 column hidden</span>
{% end %}

<!-- END AUTO-GENERATED -->

### Use cases

- **Work status** — `🚧` WIP, `✅` ready for review, `🔥` urgent
- **Agent tracking** — The [Claude Code](@/claude-code.md) plugin sets markers automatically
- **Notes** — Any short text: `"blocked"`, `"needs tests"`

### Storage

Stored in git config as `worktrunk.state.<branch>.marker`. Set directly with:

{{ terminal(cmd="git config worktrunk.state.feature.marker '{__WT_QUOT__marker__WT_QUOT__:__WT_QUOT__🚧__WT_QUOT__,__WT_QUOT__set_at__WT_QUOT__:0}'") }}

Without a subcommand, runs `get` for the current branch. For `--branch`, use `get --branch=NAME`.

### Command reference

{% terminal() %}
wt config state marker - Branch markers

Usage: <b><span class=c>wt config state marker</span></b> <span class=c>[OPTIONS]</span> <span class=c>[COMMAND]</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>get</span></b>    Get marker for a branch
  <b><span class=c>set</span></b>    Set marker for a branch
  <b><span class=c>clear</span></b>  Clear marker for a branch

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)
{% end %}

## wt config state vars

<span class="badge-experimental"></span>

Custom variables per branch.

Store custom variables per branch. Values are stored as-is — plain strings or JSON.

### Examples

Set and get values:
{{ terminal(cmd="wt config state vars set env=staging|||wt config state vars get env") }}

Store JSON:
{{ terminal(cmd="wt config state vars set config='{__WT_QUOT__port__WT_QUOT__: 3000, __WT_QUOT__debug__WT_QUOT__: true}'") }}

List all keys:
{{ terminal(cmd="wt config state vars list") }}

Operate on a different branch:
{{ terminal(cmd="wt config state vars set env=production --branch=main") }}

### Template access

Variables are available in [hook templates](@/hook.md#template-variables) as `{{ vars.<key> }}`. Use the `default` filter for keys that may not be set:

```toml
[post-start]
dev = "ENV={{ vars.env | default('development') }} npm start -- --port {{ vars.port | default('3000') }}"
```

JSON object and array values support dot access:

{{ terminal(cmd="wt config state vars set config='{__WT_QUOT__port__WT_QUOT__: 3000, __WT_QUOT__debug__WT_QUOT__: true}'") }}
```toml
[post-start]
dev = "npm start -- --port {{ vars.config.port }}"
```

### Storage format

Stored in git config as `worktrunk.state.<branch>.vars.<key>`. Keys must contain only letters, digits, hyphens, and underscores — dots conflict with git config's section separator.

### Command reference

{% terminal() %}
wt config state vars - [experimental] Custom variables per branch

Usage: <b><span class=c>wt config state vars</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>get</span></b>    Get a value
  <b><span class=c>list</span></b>   List all keys
  <b><span class=c>set</span></b>    Set a value
  <b><span class=c>clear</span></b>  Clear a key or all keys

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: hooks, templates; -vv: debug report)
{% end %}

<!-- END AUTO-GENERATED from `wt config --help-page` -->
