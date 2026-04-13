+++
title = "wt hook"
description = "Run configured hooks."
weight = 17

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt hook --help-page` — edit cli.rs to update -->

Run configured hooks.

Hooks are shell commands that run at key points in the worktree lifecycle — automatically during `wt switch`, `wt merge`, & `wt remove`, or on demand via `wt hook <type>`. Both user and project hooks are supported.

# Hook Types

| Event | `pre-` — blocking | `post-` — background |
|-------|-------------------|---------------------|
| **switch** | `pre-switch` | `post-switch` |
| **start** | `pre-start` | `post-start` |
| **commit** | `pre-commit` | `post-commit` |
| **merge** | `pre-merge` | `post-merge` |
| **remove** | `pre-remove` | `post-remove` |

`pre-*` hooks block — failure aborts the operation. `post-*` hooks run in the background with output logged (use [`wt config state logs`](@/config.md#wt-config-state-logs) to find and manage log files). Use `-v` to see expanded command details for background hooks.

The most common starting point is `post-start` — it runs background tasks (dev servers, file copying, builds) when creating a worktree.

| Hook | Purpose |
|------|---------|
| `pre-switch` | Runs before branch resolution or worktree creation. `{{ branch }}` is the destination as typed (before resolution) |
| `post-switch` | Triggers on all switch results: creating, switching to existing, or staying on current |
| `pre-start` | Tasks that must complete before `post-start`/`--execute`: dependency install, env file generation |
| `post-start` | Dev servers, long builds, file watchers, copying caches |
| `pre-commit` | Formatters, linters, type checking — runs during `wt merge` before the squash commit |
| `post-commit` | CI triggers, notifications, background linting |
| `pre-merge` | Tests, security scans, build verification — runs after rebase, before merge to target |
| `post-merge` | Deployment, notifications, installing updated binaries. Runs in the target branch worktree if it exists, otherwise the primary worktree |
| `pre-remove` | Cleanup before worktree deletion: saving test artifacts, backing up state. Runs in the worktree being removed |
| `post-remove` | Stopping dev servers, removing containers, notifying external systems. Template variables reference the removed worktree |

During `wt merge`, hooks run in this order: pre-commit → post-commit → pre-merge → pre-remove → post-remove + post-merge. As usual, post-* hooks run in the background. See [`wt merge`](@/merge.md#pipeline) for the complete pipeline.

# Security

Project commands require approval on first run:

{% terminal() %}
<span class="y">▲ <b>repo</b> needs approval to execute <b>3</b> commands:</span>

<span class="d">○</span> pre-start <b>install</b>:
<span style='background:var(--bright-white,#fff)'> </span> <span class="d"><span class="b">npm</span> ci</span>
<span class="d">○</span> pre-start <b>build</b>:
<span style='background:var(--bright-white,#fff)'> </span> <span class="d"><span class="b">cargo</span> build <span class="c">--release</span></span>
<span class="d">○</span> pre-start <b>env</b>:
<span style='background:var(--bright-white,#fff)'> </span> <span class="d"><span class="b">echo</span> <span class="g">'PORT={{ branch | hash_port }}'</span> <span class="c">></span> .env.local</span>

<span class="c">❯</span> Allow and remember? <b>[y/N]</b>
{% end %}

- Approvals are saved to `~/.config/worktrunk/approvals.toml`
- If a command changes, new approval is required
- Use `--yes` to bypass prompts — useful for CI and automation
- Use `--no-hooks` to skip hooks

Manage approvals with `wt hook approvals add` and `wt hook approvals clear`.

# Configuration

Hooks can be defined in project config (`.config/wt.toml`) or user config (`~/.config/worktrunk/config.toml`). Both use the same format. Hooks take one of three forms.

A string is a single command:

```toml
pre-start = "npm install"
```

A table is multiple commands that run concurrently:

```toml
[post-start]
server = "npm run dev"
watch = "npm run watch"
```

A pipeline is a sequence of `[[hook]]` blocks run in order. Each block is one step; multiple keys within a block run concurrently:

```toml
[[post-start]]
install = "npm ci"

[[post-start]]
build = "npm run build"
server = "npm run dev"
```

Here `install` runs first, then `build` and `server` run together.

For pre-* hooks, prefer pipeline form over table form. Table form for pre-* hooks currently runs serially rather than concurrently — this inconsistency is deprecated and will change in a future version. Using pipeline form avoids the upcoming behavior change.

## Project vs user hooks

| Aspect | Project hooks | User hooks |
|--------|--------------|------------|
| Location | `.config/wt.toml` | `~/.config/worktrunk/config.toml` |
| Scope | Single repository | All repositories (or [per-project](@/config.md#user-project-specific-settings)) |
| Approval | Required | Not required |
| Execution order | After user hooks | First |

Skip all hooks with `--no-hooks`. To run a specific hook when user and project both define the same name, use `user:name` or `project:name` syntax.

## Template variables

Hooks can use template variables that expand at runtime:

| Variable | Description |
|----------|-------------|
| `{{ branch }}` | Active branch name |
| `{{ worktree_path }}` | Active worktree path |
| `{{ worktree_name }}` | Active worktree directory name |
| `{{ commit }}` | Active branch HEAD SHA |
| `{{ short_commit }}` | Active branch HEAD SHA (7 chars) |
| `{{ upstream }}` | Active branch upstream (if tracking a remote) |
| `{{ base }}` | Base branch name |
| `{{ base_worktree_path }}` | Base worktree path |
| `{{ target }}` | Target branch name |
| `{{ target_worktree_path }}` | Target worktree path |
| `{{ cwd }}` | Directory where the hook command runs |
| `{{ repo }}` | Repository directory name |
| `{{ repo_path }}` | Absolute path to repository root |
| `{{ owner }}` | Primary remote owner path (may include subgroups) |
| `{{ primary_worktree_path }}` | Primary worktree path |
| `{{ default_branch }}` | Default branch name |
| `{{ remote }}` | Primary remote name |
| `{{ remote_url }}` | Remote URL |
| `{{ hook_type }}` | Hook type being run (e.g. `pre-start`, `pre-merge`) |
| `{{ hook_name }}` | Hook command name (if named) |
| `{{ vars.<key> }}` | Per-branch variables from [`wt config state vars`](@/config.md#wt-config-state-vars) |

Bare variables (`branch`, `worktree_path`, `commit`) refer to the branch the operation acts on: the destination for switch/create, the source for merge/remove. `base` and `target` give the other side:

| Operation | Bare vars | `base` | `target` |
|-----------|-----------|--------|----------|
| switch/create | destination | where you came from | = bare vars |
| merge | feature being merged | = bare vars | merge target |
| remove | branch being removed | = bare vars | where you end up |

Pre and post hooks share the same perspective — `{{ branch | hash_port }}` produces the same port in `post-start` and `post-remove`. `cwd` is the worktree root where the hook command runs. It differs from `worktree_path` in three cases: pre-switch, where the hook runs in the source but `worktree_path` is the destination; post-remove, where the active worktree is gone so the hook runs in primary; and post-merge with removal, same — the active worktree is gone, so the hook runs in target.

Some variables are conditional: `upstream` requires remote tracking; `base`/`target` are only in two-worktree hooks; `vars` keys may not exist. Undefined variables error — use conditionals or defaults for optional behavior:

```toml
[pre-start]
# Rebase onto upstream if tracking a remote branch (e.g., wt switch --create feature origin/feature)
sync = "{% if upstream %}git fetch && git rebase {{ upstream }}{% endif %}"
```

Variables use dot access and the `default` filter for missing keys. JSON object/array values are parsed automatically, so `{{ vars.config.port }}` works when the value is `{"port": 3000}`:

```toml
[post-start]
dev = "ENV={{ vars.env | default('development') }} npm start -- --port {{ vars.config.port | default('3000') }}"
```

## Worktrunk filters

Templates support Jinja2 filters for transforming values:

| Filter | Example | Description |
|--------|---------|-------------|
| `sanitize` | `{{ branch \| sanitize }}` | Replace `/` and `\` with `-` |
| `sanitize_db` | `{{ branch \| sanitize_db }}` | Database-safe identifier with hash suffix (`[a-z0-9_]`, max 63 chars) |
| `hash_port` | `{{ branch \| hash_port }}` | Hash to port 10000-19999 |

The `sanitize` filter makes branch names safe for filesystem paths. The `sanitize_db` filter produces database-safe identifiers — lowercase alphanumeric and underscores, no leading digits, with a 3-character hash suffix to avoid collisions and reserved words. The `hash_port` filter is useful for running dev servers on unique ports per worktree:

```toml
[post-start]
dev = "npm run dev -- --host {{ branch }}.localhost --port {{ branch | hash_port }}"
```

Hash any string, including concatenations:

```toml
# Unique port per repo+branch combination
dev = "npm run dev --port {{ (repo ~ '-' ~ branch) | hash_port }}"
```

Variables are shell-escaped automatically — quotes around `{{ ... }}` are unnecessary and can cause issues with special characters.

## Worktrunk functions

Templates also support functions for dynamic lookups:

| Function | Example | Description |
|----------|---------|-------------|
| `worktree_path_of_branch(branch)` | `{{ worktree_path_of_branch("main") }}` | Look up the path of a branch's worktree |

The `worktree_path_of_branch` function returns the filesystem path of a worktree given a branch name, or an empty string if no worktree exists for that branch. This is useful for referencing files in other worktrees:

```toml
[pre-start]
# Copy config from main worktree
setup = "cp {{ worktree_path_of_branch('main') }}/config.local {{ worktree_path }}"
```

## JSON context

Hooks receive all template variables as JSON on stdin, enabling complex logic that templates can't express:

```toml
[pre-start]
setup = "python3 scripts/pre-start-setup.py"
```

```python
import json, sys, subprocess
ctx = json.load(sys.stdin)
if ctx['branch'].startswith('feature/') and 'backend' in ctx['repo']:
    subprocess.run(['make', 'seed-db'])
```

# Running Hooks Manually

`wt hook <type>` runs hooks on demand — useful for testing during development, running in CI pipelines, or re-running after a failure.

{{ terminal(cmd="wt hook pre-merge              # Run all pre-merge hooks|||wt hook pre-merge test         # Run hooks named __WT_QUOT__test__WT_QUOT__ from both sources|||wt hook pre-merge test build   # Run hooks named __WT_QUOT__test__WT_QUOT__ and __WT_QUOT__build__WT_QUOT__|||wt hook pre-merge user:        # Run all user hooks|||wt hook pre-merge project:     # Run all project hooks|||wt hook pre-merge user:test    # Run only user's __WT_QUOT__test__WT_QUOT__ hook|||wt hook pre-merge --yes        # Skip approval prompts (for CI)|||wt hook pre-start --branch=feature/test    # Override a template variable") }}

The `user:` and `project:` prefixes filter by source. Use `user:` or `project:` alone to run all hooks from that source, or `user:name` / `project:name` to run a specific hook.

Any unknown `--KEY=VALUE` flag is treated as a template variable assignment — useful for testing hooks with different contexts without switching to that context. The long form `--var KEY=VALUE` is equivalent and remains the escape hatch when a variable name collides with a built-in flag (e.g. `config`, `yes`, `dry-run`, `foreground`, `verbose`).

# Pipeline Ordering

<span class="badge-experimental"></span>

By default, all commands in a `post-*` hook run concurrently in the background. The TOML type determines execution order. In the simplest case, a string runs one command:

```toml
post-start = "npm install"
```

Most hooks are a map of named commands, which run concurrently:

```toml
[post-start]
install = "npm install"
build = "npm run build"
lint = "npm run lint"
```

When one command depends on another — `npm run build` needs `npm install` to finish first — use `[[hook]]` blocks to run steps in order:

```toml
# Two blocks, run in order.
# Each block runs its entries concurrently.

# install runs first
[[post-start]]
install = "npm install"

# ...then build and lint run concurrently
[[post-start]]
build = "npm run build"
lint = "npm run lint"
```

In summary, the bracket count tracks the shape:

- `post-start = "npm install"` — one command
- `[post-start]` — one section of concurrent commands
- `[[post-start]]` — one of multiple sections, run in order

## How it works

Steps run in order. A failing step aborts the pipeline — later steps don't run. A multi-entry map spawns its commands concurrently and waits for all to complete before the next step.

Pre-* hooks ignore pipeline structure — all commands run serially regardless, since pre-* hooks are blocking by nature.

## When to use pipelines

Most hooks don't need pipelines. A table of concurrent post-start commands is fine when they're independent:

```toml
[post-start]
server = "npm run dev -- --port {{ branch | hash_port }}"
copy = "wt step copy-ignored"
```

Pipelines matter when there's a dependency chain — typically setup steps that must complete before other tasks can start. Common pattern: install dependencies, then run build + dev server concurrently.

# Designing Effective Hooks

## pre-start vs post-start

Both run when creating a worktree. The difference:

| Hook | Execution | Best for |
|------|-----------|----------|
| `pre-start` | Blocks until complete | Tasks the developer needs before working (dependency install) |
| `post-start` | Background, parallel | Long-running tasks that don't block worktree creation |

Many tasks work well in `post-start` — they'll likely be ready by the time they're needed, especially when the fallback is recompiling. If unsure, prefer `post-start` for faster worktree creation. For finer control over execution order within `post-start`, see [Pipeline ordering](#pipeline-ordering).

## Copying untracked files

Git worktrees share the repository but not untracked files. [`wt step copy-ignored`](@/step.md#wt-step-copy-ignored) copies gitignored files between worktrees:

```toml
[post-start]
copy = "wt step copy-ignored"
```

Use `pre-start` instead if subsequent hooks need the copied files — for example, copying `node_modules/` before `pnpm install` so the install reuses cached packages:

```toml
[[pre-start]]
copy = "wt step copy-ignored"

[[pre-start]]
install = "pnpm install"
```

## Dev servers

Run a dev server per worktree on a deterministic port using `hash_port`:

```toml
[post-start]
server = "npm run dev -- --port {{ branch | hash_port }}"

[post-remove]
server = "lsof -ti :{{ branch | hash_port }} -sTCP:LISTEN | xargs kill 2>/dev/null || true"
```

The port is stable across machines and restarts — `feature-api` always gets the same port. Show it in `wt list`:

```toml
[list]
url = "http://localhost:{{ branch | hash_port }}"
```

For subdomain-based routing (useful for cookies/CORS), use `.localhost` subdomains which resolve to 127.0.0.1:

```toml
[post-start]
server = "npm run dev -- --host {{ branch | sanitize }}.localhost --port {{ branch | hash_port }}"
```

## Databases

Each worktree can have its own database. A pipeline sets up the container name and connection string as vars, then later steps and hooks reference them:

```toml
[[post-start]]
set-vars = """
wt config state vars set \
  container='{{ repo }}-{{ branch | sanitize }}-postgres' \
  port='{{ ('db-' ~ branch) | hash_port }}' \
  db_url='postgres://postgres:dev@localhost:{{ ('db-' ~ branch) | hash_port }}/{{ branch | sanitize_db }}'
"""

[[post-start]]
db = """
docker run -d --rm \
  --name {{ vars.container }} \
  -p {{ vars.port }}:5432 \
  -e POSTGRES_DB={{ branch | sanitize_db }} \
  -e POSTGRES_PASSWORD=dev \
  postgres:16
"""

[post-remove]
db-stop = "docker stop {{ vars.container }} 2>/dev/null || true"
```

The first pipeline step derives names and ports from the branch name and stores them as vars. The second step uses `{{ vars.container }}` and `{{ vars.port }}` — expanded at execution time, after the vars are set. The `post-remove` hook reads the same vars.

The connection string is accessible anywhere — not just in hooks:

{{ terminal(cmd="DATABASE_URL=$(wt config state vars get db_url) npm start") }}

## Progressive validation

Quick checks before commit, thorough validation before merge:

```toml
[[pre-commit]]
lint = "npm run lint"

[[pre-commit]]
typecheck = "npm run typecheck"

[[pre-merge]]
test = "npm test"

[[pre-merge]]
build = "npm run build"
```

## Target-specific behavior

Different actions for production vs staging:

```toml
post-merge = """
if [ {{ target }} = main ]; then
    npm run deploy:production
elif [ {{ target }} = staging ]; then
    npm run deploy:staging
fi
"""
```

## Python virtual environments

Use `uv sync` to recreate virtual environments, or `python -m venv .venv && .venv/bin/pip install -r requirements.txt` for pip-based projects:

```toml
[pre-start]
install = "uv sync"
```

For copying dependencies and caches between worktrees, see [`wt step copy-ignored`](@/step.md#language-specific-notes).

## Hook type examples

```toml
post-merge = "cargo install --path ."

[[pre-start]]
install = "npm ci"

[[pre-start]]
env = "echo 'PORT={{ branch | hash_port }}' > .env.local"

[[pre-commit]]
format = "cargo fmt -- --check"

[[pre-commit]]
lint = "cargo clippy -- -D warnings"

[[pre-merge]]
test = "cargo test"

[[pre-merge]]
build = "cargo build --release"

[pre-switch]
pull = """
FETCH_HEAD="$(git rev-parse --git-common-dir)/FETCH_HEAD"
if [ "$(find "$FETCH_HEAD" -mmin +360 2>/dev/null)" ] || [ ! -f "$FETCH_HEAD" ]; then
    git pull
fi
"""

[post-switch]
tmux = "[ -n \"$TMUX\" ] && tmux rename-window {{ branch | sanitize }}"

[post-start]
copy = "wt step copy-ignored"
server = "npm run dev -- --port {{ branch | hash_port }}"

[post-commit]
notify = "curl -s https://ci.example.com/trigger?branch={{ branch }}"

[pre-remove]
archive = "tar -czf ~/.wt-logs/{{ branch }}.tar.gz test-results/ logs/ 2>/dev/null || true"

[post-remove]
kill-server = "lsof -ti :{{ branch | hash_port }} -sTCP:LISTEN | xargs kill 2>/dev/null || true"
remove-db = "docker stop {{ repo }}-{{ branch | sanitize }}-postgres 2>/dev/null || true"
```

## See also

- [`wt merge`](@/merge.md) — Runs hooks automatically during merge
- [`wt switch`](@/switch.md) — Runs pre-start/post-start hooks on `--create`
- [`wt config`](@/config.md) — Manage hook approvals
- [`wt config state logs`](@/config.md#wt-config-state-logs) — Access background hook logs

## Command reference

{% terminal() %}
wt hook - Run configured hooks

Usage: <b><span class=c>wt hook</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>show</span></b>         Show configured hooks
  <b><span class=c>pre-switch</span></b>   Run pre-switch hooks
  <b><span class=c>post-switch</span></b>  Run post-switch hooks
  <b><span class=c>pre-start</span></b>    Run pre-start hooks
  <b><span class=c>post-start</span></b>   Run post-start hooks
  <b><span class=c>pre-commit</span></b>   Run pre-commit hooks
  <b><span class=c>post-commit</span></b>  Run post-commit hooks
  <b><span class=c>pre-merge</span></b>    Run pre-merge hooks
  <b><span class=c>post-merge</span></b>   Run post-merge hooks
  <b><span class=c>pre-remove</span></b>   Run pre-remove hooks
  <b><span class=c>post-remove</span></b>  Run post-remove hooks
  <b><span class=c>approvals</span></b>    Manage command approvals

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

## wt hook approvals

Manage command approvals.

Project hooks require approval on first run to prevent untrusted projects from running arbitrary commands.

### Examples

Pre-approve all commands for current project:
{{ terminal(cmd="wt hook approvals add") }}

Clear approvals for current project:
{{ terminal(cmd="wt hook approvals clear") }}

Clear global approvals:
{{ terminal(cmd="wt hook approvals clear --global") }}

### How approvals work

Approved commands are saved to `~/.config/worktrunk/approvals.toml`. Re-approval is required when the command template changes or the project moves. Use `--yes` to bypass prompts in CI.

### Command reference

{% terminal() %}
wt hook approvals - Manage command approvals

Usage: <b><span class=c>wt hook approvals</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>add</span></b>    Store approvals in approvals.toml
  <b><span class=c>clear</span></b>  Clear approved commands from approvals.toml

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

<!-- END AUTO-GENERATED from `wt hook --help-page` -->
