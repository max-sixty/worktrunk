+++
title = "Configuration"
weight = 3
+++

Worktrunk uses two configuration files:

- **User config**: `~/.config/worktrunk/config.toml` — Personal settings, LLM commands, saved approvals
- **Project config**: `.config/wt.toml` — Project-specific hooks (checked into version control)

## Project Hooks

Automate setup and validation at worktree lifecycle events:

| Hook | When | Example |
|------|------|---------|
| **post-create** | After worktree created | `cp -r .cache`, `ln -s` |
| **post-start** | After worktree created (background) | `npm install`, `cargo build` |
| **pre-commit** | Before creating any commit | `pre-commit run` |
| **pre-merge** | After squash, before push | `cargo test`, `pytest` |
| **post-merge** | After successful merge | `cargo install --path .` |

### Example project config

Create `.config/wt.toml` in your repository:

```toml
# Install dependencies, build setup (blocking)
[post-create]
"install" = "uv sync"

# Dev servers, file watchers (runs in background)
[post-start]
"dev" = "uv run dev"

# Tests and lints before merging (blocks on failure)
[pre-merge]
"lint" = "uv run ruff check"
"test" = "uv run pytest"

# After merge completes
[post-merge]
"install" = "cargo install --path ."
```

### Hook execution

```bash
$ wt switch --create feature-x
🔄 Running post-create install:
   uv sync

  Resolved 24 packages in 145ms
  Installed 24 packages in 1.2s
✅ Created new worktree for feature-x from main at ../repo.feature-x
🔄 Running post-start dev:
   uv run dev
```

**Security**: Project commands require approval on first run. Approvals are saved to user config. Use `--force` to bypass prompts or `--no-verify` to skip hooks entirely.

### Template variables

Hooks can use these variables:

- `{{ repo }}` — Repository name
- `{{ branch }}` — Branch name
- `{{ worktree }}` — Worktree path
- `{{ repo_root }}` — Repository root path
- `{{ target }}` — Target branch (for merge hooks)

## LLM Commit Messages

Worktrunk can invoke external commands to generate commit messages. [llm](https://llm.datasette.io/) from Simon Willison is recommended.

### Setup

1. Install llm:
   ```bash
   $ uv tool install -U llm
   ```

2. Configure your API key:
   ```bash
   $ llm install llm-anthropic
   $ llm keys set anthropic
   ```

3. Add to user config (`~/.config/worktrunk/config.toml`):
   ```toml
   [commit-generation]
   command = "llm"
   args = ["-m", "claude-haiku-4-5-20251001"]
   ```

### Usage

`wt merge` generates commit messages automatically, or run `wt step commit` for just the commit step.

For custom prompt templates, see `wt config --help`.

## User Config Reference

Create the user config with defaults:

```bash
$ wt config create
```

This creates `~/.config/worktrunk/config.toml` with documented examples.

### Key settings

```toml
# Worktree path template
# Default: "../{{ main_worktree }}.{{ branch }}"
path-template = "../{{ main_worktree }}.{{ branch }}"

# LLM commit message generation
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4-5-20251001"]

# Per-project command approvals (auto-populated)
[approved-commands."my-project"]
"post-create.install" = "npm install"
```

## Shell Integration

Worktrunk needs shell integration to change directories. Install with:

```bash
$ wt config shell install
```

Or manually add to your shell config:

```bash
# bash/zsh
eval "$(wt config shell init bash)"

# fish
wt config shell init fish | source
```

## Environment Variables

Override default behavior with environment variables:

| Variable | Effect |
|----------|--------|
| `WORKTRUNK_CONFIG_PATH` | Override user config location (default: `~/.config/worktrunk/config.toml`) |
| `NO_COLOR` | Disable colored output |
| `CLICOLOR_FORCE` | Force colored output even when not a TTY |

These follow standard conventions — `NO_COLOR` is the [no-color.org](https://no-color.org/) standard.
