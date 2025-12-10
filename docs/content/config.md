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

Manage approvals with `wt config approvals add` to review and pre-approve commands, and `wt config approvals clear` to reset (add `--global` to clear all projects).

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

### Special variables

| Variable | Purpose |
|----------|---------|
| `WORKTRUNK_CONFIG_PATH` | Override user config file location (not a config key) |
| `NO_COLOR` | Disable colored output ([standard](https://no-color.org/)) |
| `CLICOLOR_FORCE` | Force colored output even when not a TTY |

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
#   {{ main_worktree }} - Main worktree directory name (e.g., "myproject")
#   {{ branch }}        - Branch name with slashes replaced by dashes (feature/auth → feature-auth)
#
# Paths are relative to the main worktree root (original repository directory).
#
# Example paths created (main worktree at /Users/dev/myproject, branch feature/auth):
#   "../{{ main_worktree }}.{{ branch }}" → /Users/dev/myproject.feature-auth
#   ".worktrees/{{ branch }}"             → /Users/dev/myproject/.worktrees/feature-auth
worktree-path = "../{{ main_worktree }}.{{ branch }}"

# Alternative: Inside repo (useful for bare repos)
# worktree-path = ".worktrees/{{ branch }}"

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
# Format
# - First line: <50 chars, present tense, describes WHAT and WHY (not HOW).
# - Blank line after first line.
# - Optional details with proper line breaks explaining context. Commits with more substantial changes should have more details.
# - Return ONLY the formatted message without quotes, code blocks, or preamble.
#
# Style
# - Do not give normative statements or otherwise speculate on why the change was made.
# - Broadly match the style of the previous commit messages.
#   - For example, if they're in conventional commit format, use conventional commits; if they're not, don't use conventional commits.
#
# The context contains:
# - <git-diff> with the staged changes. This is the ONLY content you should base your message on.
# - <git-info> with branch name and recent commit message titles for style reference ONLY. DO NOT use their content to inform your message.
#
# ---
# The following is the context for your task:
# ---
# <git-diff>
# ```
# {{ git_diff }}
# ```
# </git-diff>
#
# <git-info>
#   <current-branch>{{ branch }}</current-branch>
# {% if recent_commits %}
#   <previous-commit-message-titles>
# {% for commit in recent_commits %}
#     <previous-commit-message-title>{{ commit }}</previous-commit-message-title>
# {% endfor %}
#   </previous-commit-message-titles>
# {% endif %}
# </git-info>
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
# Generate a commit message that combines these changes into one cohesive message. Output only the commit message without any explanation.
#
# Format
# - First line: <50 chars, present tense, describes WHAT and WHY (not HOW).
# - Blank line after first line.
# - Optional details with proper line breaks explaining context.
# - Match the style of the existing commits being squashed (e.g., if they use conventional commits, use conventional commits).
#
# Squashing commits from {{ branch }} to merge into {{ target_branch }}
#
# Commits being combined:
# {% for commit in commits %}
# - {{ commit }}
# {% endfor %}
#
# <git-diff>
# ```
# {{ git_diff }}
# ```
# </git-diff>
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
#   {{ branch }}    - Branch name (slashes replaced with dashes)
#   {{ worktree }}  - Absolute path to the worktree
#   {{ repo_root }} - Absolute path to the repository root
#
# Merge-related hooks also support:
#   {{ target }}    - Target branch for the merge (e.g., "main")

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
# Output is logged to .git/wt-logs/{branch}-post-start-{name}.log
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

---

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


## wt config var

Variables are runtime values stored in git config, separate from
configuration files. Use `wt config show` to view file-based configuration.

### Available variables

- **default-branch**: The repository's default branch (read-only, cached)
- **marker**: Custom status marker for a branch (shown in `wt list`)

### Examples

Get the default branch:
```bash
wt config var get default-branch
```

Set a marker for current branch:
```bash
wt config var set marker "🚧 WIP"
```

Clear markers:
```bash
wt config var clear marker --all
```

---

### Command reference

```
wt config var - Get or set runtime variables (stored in git config)

Usage: wt config var [OPTIONS] <COMMAND>

Commands:
  get    Get a variable value
  set    Set a variable value
  clear  Clear a variable value

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

---

## Command reference

```
wt config - Manage configuration and shell integration

Usage: wt config [OPTIONS] <COMMAND>

Commands:
  shell      Shell integration setup
  create     Create configuration file
  show       Show configuration files & locations
  cache      Manage caches (CI status, default branch)
  var        Get or set runtime variables (stored in git config)
  approvals  Manage command approvals

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
