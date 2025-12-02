+++
title = "Configuration"
weight = 4
+++

<!-- TODO: This user config vs project config distinction is a good organizing
     principle - consider orienting more of the docs around it -->

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

<!-- ⚠️ AUTO-GENERATED-HTML from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_hooks_post_create.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> wt switch --create feature-x
🔄 <span style='color:var(--cyan,#0aa)'>Running post-create <b>install</b>:</span>
<span style='background:var(--bright-white,#fff)'> </span>  <span style='opacity:0.67'><span style='color:var(--blue,#00a)'>uv</span></span><span style='opacity:0.67'> sync</span>

  Resolved 24 packages in 145ms
  Installed 24 packages in 1.2s
✅ <span style='color:var(--green,#0a0)'>Created new worktree for <b>feature-x</b> from <b>main</b> at <b>../repo.feature-x</b></span>
🔄 <span style='color:var(--cyan,#0aa)'>Running post-start <b>dev</b>:</span>
<span style='background:var(--bright-white,#fff)'> </span>  <span style='opacity:0.67'><span style='color:var(--blue,#00a)'>uv</span></span><span style='opacity:0.67'> run dev</span>
{% end %}

<!-- END AUTO-GENERATED -->

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
