+++
title = "wt hook"
weight = 17

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt hook --help-page` — edit cli.rs to update -->

Shell commands that run at key points in the worktree lifecycle.

Hooks run automatically during `wt switch`, `wt merge`, & `wt remove`. `wt hook <type>` runs them on demand. Both user hooks (from `~/.config/worktrunk/config.toml`) and project hooks (from `.config/wt.toml`) are supported.

## Hook types

| Hook | When | Blocking | Fail-fast |
|------|------|----------|-----------|
| `post-create` | After worktree created | Yes | No |
| `post-start` | After worktree created | No (background) | No |
| `post-switch` | After every switch | No (background) | No |
| `pre-commit` | Before commit during merge | Yes | Yes |
| `pre-merge` | Before merging to target | Yes | Yes |
| `post-merge` | After successful merge | Yes | No |
| `pre-remove` | Before worktree removed | Yes | Yes |

**Blocking**: Command waits for hook to complete before continuing.
**Fail-fast**: First failure aborts the operation.

### post-create

Installing dependencies, database migrations, copying environment files.

```toml
[post-create]
install = "npm ci"
migrate = "npm run db:migrate"
env = "cp .env.example .env"
```

### post-start

Long builds, dev servers, file watchers, downloading large assets. Output logged to `.git/wt-logs/{branch}-{source}-post-start-{name}.log`.

```toml
[post-start]
build = "npm run build"
server = "npm run dev"
```

### post-switch

Triggers on all switch results: creating new worktrees, switching to existing ones, or staying on current. Useful for terminal tab renaming, tmux window names, IDE notifications. Output logged to `.git/wt-logs/{branch}-{source}-post-switch-{name}.log`.

```toml
post-switch = "echo 'Switched to {{ branch }}'"
```

### pre-commit

Formatters, linters, type checking — runs during `wt merge` before the squash commit.

```toml
[pre-commit]
format = "cargo fmt -- --check"
lint = "cargo clippy -- -D warnings"
```

### pre-merge

Tests, security scans, build verification — runs after rebase, before merge to target.

```toml
[pre-merge]
test = "cargo test"
build = "cargo build --release"
```

### post-merge

Deployment, notifications, installing updated binaries. Runs in the target branch worktree if it exists, otherwise the main worktree.

```toml
post-merge = "cargo install --path ."
```

### pre-remove

Cleanup tasks, saving state, notifying external systems.

```toml
[pre-remove]
cleanup = "rm -rf /tmp/cache/{{ branch }}"
```

During `wt merge`, hooks run in this order: pre-commit → pre-merge → pre-remove → post-merge. See [`wt merge`](@/merge.md#pipeline) for the complete pipeline.

## Security

Project commands require approval on first run:

```
▲ repo needs approval to execute 3 commands:

○ post-create install:
   echo 'Installing dependencies...'

❯ Allow and remember? [y/N]
```

- Approvals are saved to user config (`~/.config/worktrunk/config.toml`)
- If a command changes, new approval is required
- Use `--yes` to bypass prompts (useful for CI/automation)
- Use `--no-verify` to skip hooks

Manage approvals with `wt hook approvals add` and `wt hook approvals clear`.

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

### User hooks

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

Skip hooks with `--no-verify`. To run a specific hook when user and project both define the same name, use `user:name` or `project:name` syntax.

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

### Template variables

Hooks can use template variables that expand at runtime:

| Variable | Description |
|----------|-------------|
| `{{ repo }}` | Repository directory name |
| `{{ repo_path }}` | Absolute path to repository root |
| `{{ branch }}` | Branch name |
| `{{ worktree_name }}` | Worktree directory name |
| `{{ worktree_path }}` | Absolute worktree path |
| `{{ primary_worktree_path }}` | Main worktree path (for bare repos: default branch worktree) |
| `{{ default_branch }}` | Default branch name |
| `{{ commit }}` | Full HEAD commit SHA |
| `{{ short_commit }}` | Short HEAD commit SHA (7 chars) |
| `{{ remote }}` | Primary remote name |
| `{{ remote_url }}` | Remote URL |
| `{{ upstream }}` | Upstream tracking branch |
| `{{ target }}` | Target branch (merge hooks only) |
| `{{ base }}` | Base branch (creation hooks only) |
| `{{ base_worktree_path }}` | Base branch worktree (creation hooks only) |

### Worktrunk Filters

Templates support Jinja2 filters for transforming values:

| Filter | Example | Description |
|--------|---------|-------------|
| `sanitize` | `{{ branch \| sanitize }}` | Replace `/` and `\` with `-` |
| `sanitize_db` | `{{ branch \| sanitize_db }}` | Database-safe identifier with hash suffix (`[a-z0-9_]`, max 63 chars) |
| `hash_port` | `{{ branch \| hash_port }}` | Hash to port 10000-19999 |

The `sanitize` filter makes branch names safe for filesystem paths. The `sanitize_db` filter produces database-safe identifiers (lowercase alphanumeric and underscores, no leading digits, with a 3-character hash suffix to avoid collisions and reserved words). The `hash_port` filter is useful for running dev servers on unique ports per worktree:

```toml
[post-start]
dev = "npm run dev -- --host {{ branch }}.lvh.me --port {{ branch | hash_port }}"
```

Hash any string, including concatenations:

```toml
# Unique port per repo+branch combination
dev = "npm run dev --port {{ (repo ~ '-' ~ branch) | hash_port }}"
```

### Worktrunk Functions

Templates also support functions for dynamic lookups:

| Function | Example | Description |
|----------|---------|-------------|
| `worktree_path_of_branch(branch)` | `{{ worktree_path_of_branch("main") }}` | Look up the path of a branch's worktree |

The `worktree_path_of_branch` function returns the filesystem path of a worktree given a branch name, or an empty string if no worktree exists for that branch. This is useful for referencing files in other worktrees:

```toml
[post-create]
# Copy config from main worktree
setup = "cp {{ worktree_path_of_branch('main') }}/config.local {{ worktree_path }}"
```

### JSON context

Hooks also receive context as JSON on stdin, enabling hooks in any language:

```python
import json, sys
ctx = json.load(sys.stdin)
print(f"Setting up {ctx['repo']} on branch {ctx['branch']}")
```

The JSON includes all template variables plus `hook_type` and `hook_name`.

## Running hooks manually

`wt hook <type>` runs hooks on demand — useful for testing during development, running in CI pipelines, or re-running after a failure.

```bash
wt hook pre-merge              # Run all pre-merge hooks
wt hook pre-merge test         # Run hooks named "test" from both sources
wt hook pre-merge user:        # Run all user hooks
wt hook pre-merge project:     # Run all project hooks
wt hook pre-merge user:test    # Run only user's "test" hook
wt hook pre-merge project:test # Run only project's "test" hook
wt hook pre-merge --yes        # Skip approval prompts (for CI)
wt hook post-create --var branch=feature/test  # Override template variable
```

The `user:` and `project:` prefixes filter by source. Use `user:` or `project:` alone to run all hooks from that source, or `user:name` / `project:name` to run a specific hook.

The `--var KEY=VALUE` flag overrides built-in template variables — useful for testing hooks with different contexts without switching to that context.

## Designing effective hooks

### post-create vs post-start

Both run when creating a worktree. The difference:

| Hook | Execution | Best for |
|------|-----------|----------|
| `post-create` | Blocks until complete | Tasks the developer needs before working (dependency install) |
| `post-start` | Background, parallel | Long-running tasks that don't block worktree creation |

Many tasks work well in `post-start` — they'll likely be ready by the time they're needed, especially when the fallback is recompiling. If unsure, prefer `post-start` for faster worktree creation.

### Copying untracked files

Git worktrees share the repository but not untracked files (dependencies, caches, `.env`). Use [`wt step copy-ignored`](@/step.md#wt-step-copy-ignored) to copy gitignored files:

```toml
[post-create]
copy = "wt step copy-ignored"
```

See [`wt step copy-ignored`](@/step.md#wt-step-copy-ignored) for limiting what gets copied, common patterns, and language-specific notes.

### Dev servers

Run a dev server per worktree on a deterministic port using `hash_port`:

```toml
[post-start]
server = "npm run dev -- --port {{ branch | hash_port }}"
```

The port is stable across machines and restarts — `feature-api` always gets the same port. Show it in `wt list`:

```toml
[list]
url = "http://localhost:{{ branch | hash_port }}"
```

For subdomain-based routing (useful for cookies/CORS), use `lvh.me` which resolves to 127.0.0.1:

```toml
[post-start]
server = "npm run dev -- --host {{ branch | sanitize }}.lvh.me --port {{ branch | hash_port }}"
```

### Databases

Each worktree can have its own database. Docker containers get unique names and ports:

```toml
[post-start]
db = """
docker run -d --rm \
  --name {{ repo }}-{{ branch | sanitize }}-postgres \
  -p {{ ('db-' ~ branch) | hash_port }}:5432 \
  -e POSTGRES_DB={{ branch | sanitize_db }} \
  -e POSTGRES_PASSWORD=dev \
  postgres:16
"""

[pre-remove]
db-stop = "docker stop {{ repo }}-{{ branch | sanitize }}-postgres 2>/dev/null || true"
```

The `('db-' ~ branch)` concatenation hashes differently than plain `branch`, so database and dev server ports don't collide.
Jinja2's operator precedence has pipe `|` with higher precedence than concatenation `~`, meaning expressions need parentheses to filter concatenated values.

Generate `.env.local` with the connection string:

```toml
[post-create]
env = """
cat > .env.local << EOF
DATABASE_URL=postgres://postgres:dev@localhost:{{ ('db-' ~ branch) | hash_port }}/{{ branch | sanitize_db }}
DEV_PORT={{ branch | hash_port }}
EOF
"""
```

### Progressive validation

Quick checks before commit, thorough validation before merge:

```toml
[pre-commit]
lint = "npm run lint"
typecheck = "npm run typecheck"

[pre-merge]
test = "npm test"
build = "npm run build"
```

### Target-specific behavior

Different actions for production vs staging:

```toml
post-merge = """
if [ "{{ target }}" = "main" ]; then
    npm run deploy:production
elif [ "{{ target }}" = "staging" ]; then
    npm run deploy:staging
fi
"""
```

### Python virtual environments

Use `uv sync` to recreate virtual environments (or `python -m venv .venv && .venv/bin/pip install -r requirements.txt` for pip-based projects):

```toml
[post-create]
install = "uv sync"
```

For copying dependencies and caches between worktrees, see [`wt step copy-ignored`](@/step.md#language-specific-notes).

## See also

- [`wt merge`](@/merge.md) — Runs hooks automatically during merge
- [`wt switch`](@/switch.md) — Runs post-create/post-start hooks on `--create`
- [`wt config`](@/config.md) — Manage hook approvals

## Command reference

{% terminal() %}
wt hook - Run configured hooks

Usage: <b><span class=c>wt hook</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>show</span></b>         Show configured hooks
  <b><span class=c>post-create</span></b>  Run post-create hooks
  <b><span class=c>post-start</span></b>   Run post-start hooks
  <b><span class=c>post-switch</span></b>  Run post-switch hooks
  <b><span class=c>pre-commit</span></b>   Run pre-commit hooks
  <b><span class=c>pre-merge</span></b>    Run pre-merge hooks
  <b><span class=c>post-merge</span></b>   Run post-merge hooks
  <b><span class=c>pre-remove</span></b>   Run pre-remove hooks
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
          Show debug info (-v), or also write diagnostic report (-vv)
{% end %}

## wt hook approvals

Project hooks require approval on first run to prevent untrusted projects from running arbitrary commands.

### Examples

Pre-approve all commands for current project:
```bash
wt hook approvals add
```

Clear approvals for current project:
```bash
wt hook approvals clear
```

Clear global approvals:
```bash
wt hook approvals clear --global
```

### How approvals work

Approved commands are saved to user config. Re-approval is required when the command template changes or the project moves. Use `--yes` to bypass prompts in CI.

### Command reference

{% terminal() %}
wt hook approvals - Manage command approvals

Usage: <b><span class=c>wt hook approvals</span></b> <span class=c>[OPTIONS]</span> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>add</span></b>    Store approvals in config
  <b><span class=c>clear</span></b>  Clear approved commands from config

<b><span class=g>Options:</span></b>
  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Show debug info (-v), or also write diagnostic report (-vv)
{% end %}

<!-- END AUTO-GENERATED from `wt hook --help-page` -->
