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
- `{{ main_worktree }}` — main worktree directory name
- `{{ branch }}` — raw branch name (e.g., `feature/foo`)
- `{{ branch | sanitize }}` — branch name with `/` and `\` replaced by `-`

**Examples** for a repo at `~/code/myproject` creating branch `feature/login`:

```toml
# Default — siblings in parent directory
# Creates: ~/code/myproject.feature-login
worktree-path = "../{{ main_worktree }}.{{ branch | sanitize }}"

# Inside the repository
# Creates: ~/code/myproject/.worktrees/feature-login
worktree-path = ".worktrees/{{ branch | sanitize }}"

# Namespaced (useful when multiple repos share a parent directory)
# Creates: ~/code/worktrees/myproject/feature-login
worktree-path = "../worktrees/{{ main_worktree }}/{{ branch | sanitize }}"
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
| `WORKTRUNK_CONFIG_PATH` | Override user config file location |
| `WORKTRUNK_MAX_CONCURRENT_COMMANDS` | Max parallel git commands (default: 32). Lower if hitting resource limits. |
| `NO_COLOR` | Disable colored output ([standard](https://no-color.org/)) |
| `CLICOLOR_FORCE` | Force colored output even when not a TTY |

## Command reference

```
wt config - Manage configuration and shell integration

Usage: wt config [OPTIONS] <COMMAND>

Commands:
  shell   Shell integration setup
  create  Create configuration file
  show    Show configuration files & locations
  state   Get, set, or clear stored state

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info
```

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
[commit-generation]
# Example: Simon Willison's llm CLI (https://github.com/simonw/llm)
# Install: pip install llm llm-anthropic
command = "llm"
args = ["-m", "claude-haiku-4.5"]

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
#   {{ main_worktree }}     - Main worktree directory name (e.g., "myproject")
#   {{ branch }}            - Raw branch name (e.g., "feature/auth")
#   {{ branch | sanitize }} - Branch name with / and \ replaced by - (e.g., "feature-auth")
#
# Paths are relative to the main worktree root (original repository directory).
#
# Example paths created (main worktree at /Users/dev/myproject, branch feature/auth):
#   "../{{ main_worktree }}.{{ branch | sanitize }}" → /Users/dev/myproject.feature-auth
#   ".worktrees/{{ branch | sanitize }}"             → /Users/dev/myproject/.worktrees/feature-auth
worktree-path = "../{{ main_worktree }}.{{ branch | sanitize }}"

# Alternative: Inside repo (useful for bare repos)
# worktree-path = ".worktrees/{{ branch | sanitize }}"

# List Command Defaults
# Configure default behavior for `wt list`
[list]
full = false       # Show CI and `main` diffstat by default
branches = false   # Include branches without worktrees by default
remotes = false    # Include remote branches by default

# Commit Defaults (shared by `wt step commit`, `wt step squash`, and `wt merge`)
[commit]
stage = "all"          # What to stage: "all", "tracked", or "none"

# Merge Command Defaults
# Note: `stage` defaults from [commit] section above
[merge]
squash = true          # Squash commits when merging
commit = true          # Commit, squash, and rebase during merge
remove = true          # Remove worktree after merge
verify = true          # Run project hooks

# Approved Commands
# Commands approved for automatic execution after switching worktrees
# Auto-populated when you use: wt switch --execute "command" --force
[projects."github.com/user/repo"]
approved-commands = ["npm install"]

# NOTE: For project-specific hooks (post-create, post-start, pre-merge, etc.),
# use a separate PROJECT config file at <repo>/.config/wt.toml
# Run `wt config create --project` to create one, or see https://worktrunk.dev/hooks/

# ============================================================================
# Custom Prompt Templates (Advanced)
# ============================================================================
# These options belong under [commit-generation] section above.
# NOTE: Templates are synced from src/llm.rs by `cargo test readme_sync`

# Optional: Custom prompt template (inline) - Uses minijinja syntax
# Available variables: {{ git_diff }}, {{ branch }}, {{ recent_commits }}, {{ repo }}
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
# This file defines project-specific hooks that run automatically during
# worktree operations. It should be checked into git and shared across all
# developers working on the project.

# Available template variables (all hooks):
#   {{ repo }}      - Repository name (e.g., "my-project")
#   {{ branch }}    - Raw branch name (e.g., "feature/foo")
#   {{ worktree }}  - Absolute path to the worktree
#   {{ repo_root }} - Absolute path to the repository root
#
# Merge-related hooks also support:
#   {{ target }}    - Target branch for the merge (e.g., "main")
#
# Filters:
#   {{ branch | sanitize }}  - Replace / and \ with - (e.g., "feature-foo")
#   {{ branch | hash_port }} - Hash string to deterministic port (10000-19999)

# Post-Create Hook
# Runs SEQUENTIALLY and BLOCKS until complete
# The worktree switch won't complete until these finish
# Commands run one after another in the worktree directory
#
# Format options:
# 1. Single string:
#    post-create = "npm install"
#
# 2. Named table (runs sequentially in declaration order):
# [post-create]
# install = "npm install --frozen-lockfile"
# build = "npm run build"

# Post-Start Hook
# Runs in BACKGROUND as detached processes (parallel)
# Use for: uv sync, npm install, bundle install, build, dev servers, file watchers,
# downloading assets too large for git (images, ML models, binaries), long-running tasks
# The worktree switch completes immediately, these run in parallel
# Output is logged to .git/wt-logs/{branch}-{source}-post-start-{name}.log (source: user/project)
#
# Format options:
# 1. Single string:
#    post-start = "npm run dev"
#
# 2. Named table (runs in parallel):
# [post-start]
# server = "npm run dev"
# watch = "npm run watch"

# Pre-Commit Hook
# Runs SEQUENTIALLY before committing changes during merge (blocking, fail-fast)
# All commands must exit with code 0 for commit to proceed
# Runs for both squash and no-squash merge modes
# Use for: formatters, linters, quick validation
#
# Single command:
# pre-commit = "cargo fmt -- --check"
#
# Multiple commands:
# [pre-commit]
# format = "cargo fmt -- --check"
# lint = "cargo clippy -- -D warnings"

# Pre-Merge Hook
# Runs SEQUENTIALLY before merging to target branch (blocking, fail-fast)
# All commands must exit with code 0 for merge to proceed
# Use for: tests, linters, build verification before merging
#
# Single command:
# pre-merge = "cargo test"
#
# Multiple commands:
# [pre-merge]
# test = "cargo test"
# build = "cargo build --release"

# Post-Merge Hook
# Runs SEQUENTIALLY in the main worktree after successful merge (blocking)
# Runs after push and cleanup complete
# Use for: updating production builds, notifications, cleanup
#
# Single command:
# post-merge = "cargo install --path ."
#
# Multiple commands:
# [post-merge]
# install = "cargo install --path ."
# notify = "echo 'Merged!'"

# Example: Node.js Project
# [post-create]
# install = "npm ci"
#
# [post-start]
# server = "npm run dev"
#
# [pre-merge]
# lint = "npm run lint"
# test = "npm test"

# Example: Rust Project
# [post-create]
# build = "cargo build"
#
# [pre-merge]
# format = "cargo fmt -- --check"
# clippy = "cargo clippy -- -D warnings"
# test = "cargo test"
#
# post-merge = "cargo install --path ."

# Example: Python Project
# [post-create]
# venv = "python -m venv .venv"
# install = ".venv/bin/pip install -r requirements.txt"
#
# [pre-merge]
# format = ".venv/bin/black --check ."
# lint = ".venv/bin/ruff check ."
# test = ".venv/bin/pytest"
```

### Command reference

```
wt config create - Create configuration file

Usage: wt config create [OPTIONS]

Options:
      --project
          Create project config (.config/wt.toml) instead of user config

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info
```


## wt config state

State is stored in `.git/` (config entries and log files), separate from configuration files.
Use `wt config show` to view file-based configuration.

### Keys

- **default-branch**: The repository's default branch (main, master, etc.)
- **previous-branch**: Previous branch for `wt switch -`
- **ci-status**: CI/PR status for a branch (passed, running, failed, conflicts, noci)
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

```
wt config state - Get, set, or clear stored state

Usage: wt config state [OPTIONS] <COMMAND>

Commands:
  default-branch   Default branch setting
  previous-branch  Previous branch (for wt switch -)
  ci-status        CI status cache
  marker           Branch markers
  logs             Background operation logs
  get              Get all stored state
  clear            Clear all stored state

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info
```

## wt config state default-branch

### Detection

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

### When to use

Most users never need this command — default branch detection is automatic.
Use it to:

- **Debug** — See what worktrunk thinks the default branch is
- **Override** — Set a non-standard default branch with `set`
- **Refresh** — Force re-query with `get --refresh` after remote changes
- **Clear** — Remove cached value with `clear`

Without a subcommand, runs `get`. For `--refresh`, use `get --refresh`.

### Command reference

```
wt config state default-branch - Default branch setting

Usage: wt config state default-branch [OPTIONS] [COMMAND]

Commands:
  get    Get the default branch
  set    Set the default branch
  clear  Clear the default branch cache

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info
```


## wt config state ci-status

Caches GitHub/GitLab CI status for display in [wt list](@/list.md#ci-status).

### How it works

1. **Platform detection** — Detected from remote URL (github.com → GitHub, gitlab.com → GitLab)
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
| `noci` | No checks configured |

See [wt list CI status](@/list.md#ci-status) for display symbols and colors.

### When to use

- **Debug** — See cached status and when it was fetched
- **Refresh** — Force re-fetch with `get --refresh`
- **Clear** — Remove stale cache entries

Without a subcommand, runs `get` for the current branch. For `--branch` or `--refresh`, use `get --branch=NAME`.

### Command reference

```
wt config state ci-status - CI status cache

Usage: wt config state ci-status [OPTIONS] [COMMAND]

Commands:
  get    Get CI status for a branch
  clear  Clear CI status cache

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info
```


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

```
wt config state marker - Branch markers

Usage: wt config state marker [OPTIONS] [COMMAND]

Commands:
  get    Get marker for a branch
  set    Set marker for a branch
  clear  Clear marker for a branch

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info
```


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

```
wt config state logs - Background operation logs

Usage: wt config state logs [OPTIONS] [COMMAND]

Commands:
  get    List background operation log files
  clear  Clear background operation logs

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info
```

<!-- END AUTO-GENERATED from `wt config --help-page` -->
