+++
title = "wt config"
weight = 15

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt config --help-page` — edit src/cli.rs to update -->

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

Create project config file (.config/wt.toml) for hooks:

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

---

## Command reference

<!-- ⚠️ AUTO-GENERATED from `wt config --help-page` — edit cli.rs to update -->

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
