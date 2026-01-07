+++
title = "wt config"
weight = 15

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt config --help-page` — edit cli.rs to update -->

Manages configuration, shell integration, and runtime settings.

Worktrunk uses two configuration files:

| File | Location | Purpose |
|------|----------|---------|
| **User config** | `~/.config/worktrunk/config.toml` | Personal settings, command defaults, approved project commands |
| **Project config** | `.config/wt.toml` | Lifecycle hooks, checked into version control |

## Examples

Install shell integration (required for directory switching):

```bash
wt config shell install
```

Create user config file with documented examples:

```bash
wt config create
```

Create project config file (`.config/wt.toml`) for hooks:

```bash
wt config create --project
```

Show current configuration and file locations:

```bash
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

URLs are dimmed when the port isn't listening. The template supports `{{ branch }}` with filters `hash_port` (port 10000-19999) and `sanitize` (filesystem-safe).

### CI platform override

The `[ci]` section overrides CI platform detection for GitHub Enterprise or self-hosted GitLab with custom domains:

```toml
[ci]
platform = "github"  # or "gitlab"
```

By default, the platform is detected from the remote URL. Use this when URL detection fails (e.g., `git.mycompany.com` instead of `github.mycompany.com`).

## Shell integration

Worktrunk needs shell integration to change directories when switching worktrees. Install with:

```bash
wt config shell install
```

Or manually add to the shell config:

```bash
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

```bash
export WORKTRUNK_COMMIT_GENERATION__ARGS="-m claude-haiku"
```

### Example: CI/testing override

Override the LLM command in CI to use a mock:

```bash
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

## Command reference

{% terminal() %}
wt config - Manage configuration and shell integration

Usage: <b><span class=c>wt config</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>shell</span></b>   Shell integration setup
  <b><span class=c>create</span></b>  Create configuration file
  <b><span class=c>show</span></b>    Show configuration files &amp; locations
  <b><span class=c>state</span></b>   Manage internal data and cache

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b>
          Show commands and debug info
{% end %}

## wt config create

### User config

Creates `~/.config/worktrunk/config.toml` with the following content:

```
# Worktrunk Global Configuration
# Copy to: ~/.config/worktrunk/config.toml (or use `wt config create`)
#
# Alternative locations (XDG Base Directory spec):
#   macOS/Linux:   $XDG_CONFIG_HOME/worktrunk/config.toml
#   Windows:       %APPDATA%\worktrunk\config.toml

# Commit Message Generation (Optional)
# For generating commit messages during merge operations (wt merge)
# [commit-generation]
# Example: Simon Willison's llm CLI (https://github.com/simonw/llm)
# Install: pip install llm llm-anthropic
# command = "llm"
# args = ["-m", "claude-haiku-4.5"]

# Alternative: AIChat - Rust-based, supports 20+ providers
# Install from: https://github.com/sigoden/aichat
# command = "aichat"
# args = ["-m", "claude:claude-haiku-4.5"]

# Optional: Load template from file (mutually exclusive with 'template')
# Supports ~ expansion: ~/.config/worktrunk/commit-template.txt
# template-file = "~/.config/worktrunk/commit-template.txt"

# Optional: Load squash template from file (mutually exclusive with 'squash-template')
# Supports ~ expansion: ~/.config/worktrunk/squash-template.txt
# squash-template-file = "~/.config/worktrunk/squash-template.txt"

# See "Custom Prompt Templates" section at end of file for inline template options.

# Worktree Path Template
# Variables:
#   {{ repo }}              - Repository directory name (e.g., "myproject")
#   {{ branch }}            - Raw branch name (e.g., "feature/auth")
#   {{ branch | sanitize }} - Branch name with / and \ replaced by - (e.g., "feature-auth")
#
# Paths are relative to the main worktree root (original repository directory).
#
# Example paths created (repo at /Users/dev/myproject, branch feature/auth):
#   "../{{ repo }}.{{ branch | sanitize }}" → /Users/dev/myproject.feature-auth
#   ".worktrees/{{ branch | sanitize }}"    → /Users/dev/myproject/.worktrees/feature-auth
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

# Alternative: Inside repo (useful for bare repos)
# worktree-path = ".worktrees/{{ branch | sanitize }}"

# List Command Defaults
# Configure default behavior for `wt list`
[list]
full = false       # Show CI and default-branch merge-base diffstat (`main…±` column) by default
branches = false   # Include branches without worktrees by default
remotes = false    # Include remote branches by default

# Commit Defaults (shared by `wt step commit`, `wt step squash`, and `wt merge`)
[commit]
stage = "all"          # What to stage: "all", "tracked", or "none"

# Merge Command Defaults
# Note: `stage` defaults from [commit] section above
[merge]
squash = true          # Squash commits when merging
commit = true          # Commit uncommitted changes during merge (disables squash when false)
rebase = true          # Rebase onto target before merging
remove = true          # Remove worktree after merge
verify = true          # Run project hooks

# Select Command Defaults
# Configure pager behavior for `wt select` diff previews
[select]
# Pager command with flags for diff preview (overrides git's core.pager)
# Use this to specify pager flags needed for non-TTY contexts
# Example: pager = "delta --paging=never"

# Approved Commands
# Commands approved for project hooks in this repo
# Auto-populated when approving hooks (prompt on first run) or via `wt hook approvals add`
[projects."github.com/user/repo"]
approved-commands = ["npm ci", "npm test"]

# NOTE: For project-specific hooks (post-create, post-start, pre-merge, etc.),
# use a separate PROJECT config file at <repo>/.config/wt.toml
# Run `wt config create --project` to create one, or see https://worktrunk.dev/hook/

# ============================================================================
# Custom Prompt Templates (Advanced)
# ============================================================================
# These options belong under [commit-generation] section above.
# NOTE: Templates are synced from src/llm.rs by `cargo test readme_sync`

# Optional: Custom prompt template (inline) - Uses minijinja syntax
# Available variables: {{ git_diff }}, {{ git_diff_stat }}, {{ branch }}, {{ recent_commits }}, {{ repo }}
# If not specified, uses the default template shown below:
# <!-- DEFAULT_TEMPLATE_START -->
# template = """
# Write a commit message for the staged changes below.
#
# <format>
# - Subject under 50 chars, blank line, then optional body
# - Output only the commit message, no quotes or code blocks
# </format>
#
# <style>
# - Imperative mood: "Add feature" not "Added feature"
# - Match recent commit style (conventional commits if used)
# - Describe the change, not the intent or benefit
# </style>
#
# <diffstat>
# {{ git_diff_stat }}
# </diffstat>
#
# <diff>
# {{ git_diff }}
# </diff>
#
# <context>
# Branch: {{ branch }}
# {% if recent_commits %}<recent_commits>
# {% for commit in recent_commits %}- {{ commit }}
# {% endfor %}</recent_commits>{% endif %}
# </context>
# """
# <!-- DEFAULT_TEMPLATE_END -->
#
# Example alternative template with different style:
# template = """
# Generate a commit message for {{ repo | upper }}.
#
# Branch: {{ branch }}
# {%- if recent_commits %}
#
# Recent commit style ({{ recent_commits | length }} commits):
# {%- for commit in recent_commits %}
#   {{ loop.index }}. {{ commit }}
# {%- endfor %}
# {%- endif %}
#
# Changes to commit:
# ```
# {{ git_diff }}
# ```
#
# Requirements:
# - Follow the style of recent commits above
# - First line under 50 chars
# - Focus on WHY, not HOW
# """

# Optional: Custom squash commit message template (inline) - Uses minijinja syntax
# Available variables: {{ git_diff }}, {{ branch }}, {{ recent_commits }}, {{ repo }}, {{ commits }}, {{ target_branch }}
# If not specified, uses the default template:
# <!-- DEFAULT_SQUASH_TEMPLATE_START -->
# squash-template = """
# Combine these commits into a single commit message.
#
# <format>
# - Subject under 50 chars, blank line, then optional body
# - Output only the commit message, no quotes or code blocks
# </format>
#
# <style>
# - Imperative mood: "Add feature" not "Added feature"
# - Match the style of commits being squashed (conventional commits if used)
# - Describe the change, not the intent or benefit
# </style>
#
# <commits branch="{{ branch }}" target="{{ target_branch }}">
# {% for commit in commits %}- {{ commit }}
# {% endfor %}</commits>
#
# <diffstat>
# {{ git_diff_stat }}
# </diffstat>
#
# <diff>
# {{ git_diff }}
# </diff>
# """
# <!-- DEFAULT_SQUASH_TEMPLATE_END -->
#
# Example alternative template:
# squash-template = """
# Squashing {{ commits | length }} commit(s) from {{ branch }} to {{ target_branch }}.
#
# {% if commits | length > 1 -%}
# Commits being combined:
# {%- for c in commits %}
#   {{ loop.index }}/{{ loop.length }}: {{ c }}
# {%- endfor %}
# {%- else -%}
# Single commit: {{ commits[0] }}
# {%- endif %}
#
# Generate one cohesive commit message that captures the overall change.
# Use conventional commit format (feat/fix/docs/refactor).
# """
```

### Project config

With `--project`, creates `.config/wt.toml` in the current repository:

```
# Worktrunk Project Configuration
# Copy to: <repo>/.config/wt.toml
#
# Project-specific hooks that run automatically during worktree operations.
# Check this file into git to share hooks across all developers.

# ============================================================================
# Hook Formats
# ============================================================================
# All hooks support two formats:
#
# Single command:
#   post-create = "npm install"
#
# Named table (multiple commands):
#   [post-create]
#   deps = "npm install"
#   build = "npm run build"
#
# Named commands appear in output, making it easier to identify failures.

# ============================================================================
# Template Variables
# ============================================================================
# All hooks support these variables:
#   {{ repo }}               - Repository directory name (e.g., "myproject")
#   {{ branch }}             - Branch name (e.g., "feature/auth")
#   {{ worktree_path }}      - Absolute path to worktree
#   {{ main_worktree_path }} - Absolute path to main worktree
#   {{ default_branch }}     - Default branch name (e.g., "main")
#
# Merge hooks (pre-commit, pre-merge, post-merge) also support:
#   {{ target }}             - Target branch for the merge
#
# Filters:
#   {{ branch | sanitize }}  - Replace / and \ with - (e.g., "feature-auth")
#   {{ branch | hash_port }} - Deterministic port 10000-19999

# ============================================================================
# Hooks
# ============================================================================

# Post-Create: Runs after worktree creation, BLOCKS until complete
# Use for: installing dependencies, setting up databases, copying configs
#
# [post-create]
# deps = "npm ci"
# env = "cp .env.example .env"

# Post-Start: Runs in BACKGROUND after worktree creation (parallel)
# Use for: dev servers, file watchers, background builds
# Output logged to .git/wt-logs/{branch}-project-post-start-{name}.log
#
# [post-start]
# server = "npm run dev -- --port {{ branch | hash_port }}"
# watch = "npm run watch"

# Post-Switch: Runs in BACKGROUND after every switch (parallel)
# Use for: terminal tab naming, tmux window titles, IDE notifications
#
# post-switch = "echo -ne '\\033]0;{{ branch }}\\007'"

# Pre-Commit: Runs before committing during merge, BLOCKS (fail-fast)
# Use for: formatters, linters, quick checks
#
# [pre-commit]
# format = "npm run format:check"
# lint = "npm run lint"

# Pre-Merge: Runs before merging to target, BLOCKS (fail-fast)
# Use for: tests, build verification
#
# [pre-merge]
# test = "npm test"
# build = "npm run build"

# Post-Merge: Runs after successful merge, BLOCKS
# Use for: deployment, notifications
#
# post-merge = "echo 'Merged {{ branch }} to {{ target }}'"

# Pre-Remove: Runs before worktree removal, BLOCKS (fail-fast)
# Use for: cleanup, stopping services
#
# pre-remove = "docker compose down"

# ============================================================================
# Dev Server URL (shown in `wt list`)
# ============================================================================
# [list]
# url = "http://localhost:{{ branch | hash_port }}"

# ============================================================================
# CI Platform Override
# ============================================================================
# Override CI platform detection for GitHub Enterprise or self-hosted GitLab
# with custom domains where URL detection fails.
#
# [ci]
# platform = "github"  # or "gitlab"
```

### Command reference

{% terminal() %}
wt config create - Create configuration file

Usage: <b><span class=c>wt config create</span></b> <span class=c>[OPTIONS]</span>

<b><span class=g>Options:</span></b>
      <b><span class=c>--project</span></b>
          Create project config (<b>.config/wt.toml</b>) instead of user config

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b>
          Show commands and debug info
{% end %}


## wt config show

Shows location and contents of user config (`~/.config/worktrunk/config.toml`)
and project config (`.config/wt.toml`).

If a config file doesn't exist, shows defaults that would be used.

### Full diagnostics

Use `--full` to run diagnostic checks:

```bash
wt config show --full
```

This tests:
- **CI tool status** — Whether `gh` (GitHub) or `glab` (GitLab) is installed and authenticated
- **Commit generation** — Whether the LLM command can generate commit messages

### Command reference

{% terminal() %}
wt config show - Show configuration files &amp; locations

Usage: <b><span class=c>wt config show</span></b> <span class=c>[OPTIONS]</span>

<b><span class=g>Options:</span></b>
      <b><span class=c>--full</span></b>
          Run diagnostic checks (CI tools, commit generation)

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b>
          Show commands and debug info
{% end %}


## wt config state

State is stored in `.git/` (config entries and log files), separate from configuration files.
Use `wt config show` to view file-based configuration.

### Keys

- **default-branch**: The repository's default branch (main, master, etc.)
- **previous-branch**: Previous branch for `wt switch -`
- **ci-status**: CI/PR status for a branch (passed, running, failed, conflicts, no-ci, error)
- **marker**: Custom status marker for a branch (shown in `wt list`)
- **logs**: Background operation logs

### Examples

Get the default branch:
```bash
wt config state default-branch
```

Set the default branch manually:
```bash
wt config state default-branch set main
```

Set a marker for current branch:
```bash
wt config state marker set "🚧 WIP"
```

Clear all CI status cache:
```bash
wt config state ci-status clear --all
```

Show all stored state:
```bash
wt config state get
```

Clear all stored state:
```bash
wt config state clear
```

### Command reference

{% terminal() %}
wt config state - Manage internal data and cache

Usage: <b><span class=c>wt config state</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>default-branch</span></b>   Default branch setting
  <b><span class=c>previous-branch</span></b>  Previous branch (for <b>wt switch -</b>)
  <b><span class=c>ci-status</span></b>        CI status cache
  <b><span class=c>marker</span></b>           Branch markers
  <b><span class=c>logs</span></b>             Background operation logs
  <b><span class=c>hints</span></b>            One-time hints shown in this repo
  <b><span class=c>get</span></b>              Get all stored state
  <b><span class=c>clear</span></b>            Clear all stored state

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b>
          Show commands and debug info
{% end %}

## wt config state default-branch

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
- Looks for common names: main, master, develop, trunk

### When to use

Most users never need this command — default branch detection is automatic.
Use it to:

- **Debug** — See what worktrunk thinks the default branch is
- **Override** — Set a non-standard default branch with `set`
- **Refresh** — Force re-query with `get --refresh` after remote changes
- **Clear** — Remove cached value with `clear`

Without a subcommand, runs `get`. For `--refresh`, use `get --refresh`.

### Command reference

{% terminal() %}
wt config state default-branch - Default branch setting

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

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b>
          Show commands and debug info
{% end %}


## wt config state ci-status

Caches GitHub/GitLab CI status for display in [`wt list`](@/list.md#ci-status).

### How it works

1. **Platform detection** — From `[ci] platform` in project config, or detected from remote URL (github.com → GitHub, gitlab.com → GitLab)
2. **CLI requirement** — Requires `gh` (GitHub) or `glab` (GitLab) CLI, authenticated
3. **What's checked** — PRs/MRs first, then branch pipelines for branches with upstream
4. **Caching** — Results cached 30-60 seconds per branch+commit

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

### When to use

- **Debug** — See cached status and when it was fetched
- **Refresh** — Force re-fetch with `get --refresh`
- **Clear** — Remove stale cache entries

Without a subcommand, runs `get` for the current branch. For `--branch` or `--refresh`, use `get --branch=NAME`.

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

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b>
          Show commands and debug info
{% end %}


## wt config state marker

Custom status text or emoji shown in the `wt list` Status column.

### Display

Markers appear at the start of the Status column:

```
Branch    Status   Path
main      ^        ~/code/myproject
feature   🚧↑      ~/code/myproject.feature
bugfix    🤖!↑⇡    ~/code/myproject.bugfix
```

### Use cases

- **Work status** — `🚧` WIP, `✅` ready for review, `🔥` urgent
- **Agent tracking** — The [Claude Code plugin](@/claude-code.md) sets markers automatically
- **Notes** — Any short text: `"blocked"`, `"needs tests"`

### Storage

Stored in git config as `worktrunk.state.<branch>.marker`. Set directly with:

```bash
git config worktrunk.state.feature.marker '{"marker":"🚧","set_at":0}'
```

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

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b>
          Show commands and debug info
{% end %}


## wt config state logs

View and manage logs from background operations.

### What's logged

| Operation | Log file |
|-----------|----------|
| post-start hooks | `{branch}-{source}-post-start-{name}.log` |
| Background removal | `{branch}-remove.log` |

Source is `user` or `project` depending on where the hook is defined.

### Location

All logs are stored in `.git/wt-logs/` (in the main worktree's git directory).

### Behavior

- **Overwrites** — Same operation on same branch overwrites previous log
- **Persists** — Logs from deleted branches remain until manually cleared
- **Shared** — All worktrees write to the same log directory

### Examples

List all log files:
```bash
wt config state logs get
```

View a specific log:
```bash
cat "$(git rev-parse --git-dir)/wt-logs/feature-project-post-start-build.log"
```

Clear all logs:
```bash
wt config state logs clear
```

### Command reference

{% terminal() %}
wt config state logs - Background operation logs

Usage: <b><span class=c>wt config state logs</span></b> <span class=c>[OPTIONS]</span> <span class=c>[COMMAND]</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>get</span></b>    List background operation log files
  <b><span class=c>clear</span></b>  Clear background operation logs

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b>
          Show commands and debug info
{% end %}

<!-- END AUTO-GENERATED from `wt config --help-page` -->
