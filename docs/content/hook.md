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

Runs after worktree creation, **blocks until complete**. The worktree switch doesn't finish until these commands succeed.

**Use cases**: Installing dependencies, database migrations, copying environment files.

```toml
[post-create]
install = "npm ci"
migrate = "npm run db:migrate"
env = "cp .env.example .env"
```

### post-start

Runs after worktree creation, **in background**. The worktree switch completes immediately; these run in parallel.

**Use cases**: Long builds, dev servers, file watchers, downloading large assets.

```toml
[post-start]
build = "npm run build"
server = "npm run dev"
```

Output logged to `.git/wt-logs/{branch}-{source}-post-start-{name}.log` (source is `user` or `project`).

### post-switch

Runs after **every** switch operation, **in background**. Triggers on all switch results: creating new worktrees, switching to existing ones, or switching to the current worktree.

**Use cases**: Renaming terminal tabs, updating tmux window names, IDE notifications.

```toml
post-switch = "echo 'Switched to {{ branch }}'"
```

Output logged to `.git/wt-logs/{branch}-{source}-post-switch-{name}.log` (source is `user` or `project`).

### pre-commit

Runs before committing during `wt merge`, **fail-fast**. All commands must exit 0 for the commit to proceed.

**Use cases**: Formatters, linters, type checking.

```toml
[pre-commit]
format = "cargo fmt -- --check"
lint = "cargo clippy -- -D warnings"
```

### pre-merge

Runs before merging to target branch, **fail-fast**. All commands must exit 0 for the merge to proceed.

**Use cases**: Tests, security scans, build verification.

```toml
[pre-merge]
test = "cargo test"
build = "cargo build --release"
```

### post-merge

Runs after successful merge in the **worktree for the target branch** if it exists, otherwise the **main worktree**, **best-effort**. Failures are logged but don't abort.

**Use cases**: Deployment, notifications, installing updated binaries.

```toml
post-merge = "cargo install --path ."
```

### pre-remove

Runs before worktree removal during `wt remove`, **fail-fast**. All commands must exit 0 for removal to proceed.

**Use cases**: Cleanup tasks, saving state, notifying external systems.

```toml
[pre-remove]
cleanup = "rm -rf /tmp/cache/{{ branch }}"
```

### Timing during merge

- **pre-commit** — After staging, before squash commit
- **pre-merge** — After rebase, before merge to target
- **pre-remove** — Before removing worktree during cleanup
- **post-merge** — After cleanup completes

See [`wt merge`](@/merge.md#pipeline) for the complete pipeline.

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

### Template variables

Hooks can use template variables that expand at runtime:

| Variable | Example | Description |
|----------|---------|-------------|
| `{{ repo }}` | myproject | Repository directory name |
| `{{ repo_path }}` | /path/to/myproject | Absolute path to repository root |
| `{{ branch }}` | feature/auth | Branch name |
| `{{ worktree_name }}` | myproject.feature-auth | Worktree directory name |
| `{{ worktree_path }}` | /path/to/myproject.feature-auth | Absolute worktree path |
| `{{ main_worktree_path }}` | /path/to/myproject | Default branch worktree |
| `{{ default_branch }}` | main | Default branch name |
| `{{ commit }}` | a1b2c3d4e5f6... | Full HEAD commit SHA |
| `{{ short_commit }}` | a1b2c3d | Short HEAD commit SHA |
| `{{ remote }}` | origin | Primary remote name |
| `{{ remote_url }}` | git@github.com:user/repo.git | Remote URL |
| `{{ upstream }}` | origin/feature | Upstream tracking branch |
| `{{ target }}` | main | Target branch (merge hooks only) |
| `{{ base }}` | main | Base branch (creation hooks only) |
| `{{ base_worktree_path }}` | /path/to/myproject | Base branch worktree (creation hooks only) |

See [Designing effective hooks](#designing-effective-hooks) for `main_worktree_path` patterns.

**Deprecated:** `repo_root` (use `repo_path`), `worktree` (use `worktree_path`), `main_worktree` (use `repo`). These still work but emit warnings.

### Filters

Templates support Jinja2 filters for transforming values:

| Filter | Example | Description |
|--------|---------|-------------|
| `sanitize` | `{{ branch \| sanitize }}` | Replace `/` and `\` with `-` |
| `hash_port` | `{{ branch \| hash_port }}` | Hash to port 10000-19999 |

The `sanitize` filter makes branch names safe for filesystem paths. The `hash_port` filter is useful for running dev servers on unique ports per worktree:

```toml
[post-start]
dev = "npm run dev -- --host {{ branch }}.lvh.me --port {{ branch | hash_port }}"
```

Hash any string, including concatenations:

```toml
# Unique port per repo+branch combination
dev = "npm run dev --port {{ (repo ~ '-' ~ branch) | hash_port }}"
```

### JSON context

Hooks also receive context as JSON on stdin, enabling hooks in any language:

```python
import json, sys
ctx = json.load(sys.stdin)
print(f"Setting up {ctx['repo']} on branch {ctx['branch']}")
```

The JSON includes all template variables plus `hook_type` and `hook_name`.

## Designing effective hooks

### post-create vs post-start

Both run when creating a worktree. The difference:

| Hook | Execution | Best for |
|------|-----------|----------|
| `post-create` | Blocks until complete | Tasks the developer needs before working (dependency install) |
| `post-start` | Background, parallel | Long-running tasks that can finish while you work |

Many tasks work well in `post-start` — they'll likely be ready by the time you need them, especially when the fallback is recompiling. If unsure, prefer `post-start` for faster worktree creation.

### Copying untracked files

Git worktrees share the repository but not untracked files. Common files to copy or link:

- **Dependencies**: `node_modules/`, `.venv/`, `target/`, `vendor/`, `Pods/`
- **Build caches**: `.cache/`, `.next/`, `.parcel-cache/`, `.turbo/`
- **Generated assets**: Images, ML models, binaries too large for git
- **Environment files**: `.env` (if not generated per-worktree)

Three strategies:

| Strategy | Speed | Disk | State |
|----------|-------|------|-------|
| **Symlink** | Instant | Shared | Shared (changes affect all worktrees) |
| **Copy** | Slow | Full duplicate | Isolated |
| **Copy-on-write** | Instant | Shared until modified | Isolated |

### Copy-on-write for disk efficiency

Copy-on-write (CoW) creates instant clones that share disk space until modified. Perfect for large directories.

| OS | Filesystem | Command |
|----|------------|---------|
| macOS | APFS | `cp -c` |
| Linux | Btrfs, XFS | `cp --reflink=auto` |
| Windows | ReFS | No shell equivalent (requires API) |

Cross-platform pattern that auto-detects and falls back gracefully:

```toml
[post-create]
deps = """
if cp -c /dev/null /dev/null 2>/dev/null; then
    cp -c -r {{ main_worktree_path }}/node_modules .
elif cp --reflink=auto /dev/null /dev/null 2>/dev/null; then
    cp --reflink=auto -r {{ main_worktree_path }}/node_modules .
else
    cp -r {{ main_worktree_path }}/node_modules .
fi
"""
```

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
  -e POSTGRES_DB={{ repo }} \
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
DATABASE_URL=postgres://postgres:dev@localhost:{{ ('db-' ~ branch) | hash_port }}/{{ repo }}
DEV_PORT={{ branch | hash_port }}
EOF
"""
```

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

## User hooks

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
```

The `user:` and `project:` prefixes filter by source. Use `user:` or `project:` alone to run all hooks from that source, or `user:name` / `project:name` to run a specific hook.

## Language-specific tips

Each ecosystem has quirks that affect hook design. Contributions welcome for languages not listed.

### Rust

The `target/` directory is huge (often 1-10GB) and benefits most from CoW. Worktrunk's own config seeds compiled dependencies:

```toml
[post-start]
deps = """
[ -d {{ main_worktree_path }}/target/debug/deps ] && [ ! -e target ] &&
mkdir -p target/debug/deps &&
cp -c {{ main_worktree_path }}/target/debug/deps/*.rlib {{ main_worktree_path }}/target/debug/deps/*.rmeta target/debug/deps/ &&
cp -cR {{ main_worktree_path }}/target/debug/.fingerprint {{ main_worktree_path }}/target/debug/build target/debug/
"""
```

This cuts first build from ~68s to ~3s by reusing compiled dependencies. The check `[ ! -e target ]` skips if target already exists.

### Python

Virtual environments contain absolute paths and can't be copied between directories. Use `uv sync` to recreate — it's fast enough that copying isn't worth it:

```toml
[post-create]
install = "uv sync"
```

For pip-based projects without uv:

```toml
[post-create]
venv = "python -m venv .venv && .venv/bin/pip install -r requirements.txt"
```

### Node.js

`node_modules/` is large but mostly static. CoW works well:

```toml
[post-create]
deps = "cp -c -r {{ main_worktree_path }}/node_modules . 2>/dev/null || npm ci"
```

If the project has no native dependencies, symlinks are even faster:

```toml
[post-create]
deps = "ln -sf {{ main_worktree_path }}/node_modules ."
```

### Hook flow patterns

**Progressive validation** — Quick checks before commit, thorough validation before merge:

```toml
[pre-commit]
lint = "npm run lint"
typecheck = "npm run typecheck"

[pre-merge]
test = "npm test"
build = "npm run build"
```

**Target-specific behavior** — Different actions for production vs staging:

```toml
post-merge = """
if [ "{{ target }}" = "main" ]; then
    npm run deploy:production
elif [ "{{ target }}" = "staging" ]; then
    npm run deploy:staging
fi
"""
```

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

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b>
          Show commands and debug info
{% end %}

## wt hook approvals

### How Approvals Work

Commands from project hooks (`.config/wt.toml`) require approval on first run.
This prevents untrusted projects from running arbitrary commands.

**Approval flow:**
1. Command is shown as a template (variables not expanded)
2. User approves or denies
3. Approved commands are saved to user config under `[projects."project-id"]`

**When re-approval is required:**
- Command template changes (not just variable values)
- Project ID changes (repository moves)

**Bypassing prompts:**
- `--yes` flag on individual commands (e.g., `wt merge --yes`)
- Useful for CI/automation where prompts aren't possible

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

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b>
          Show commands and debug info
{% end %}

<!-- END AUTO-GENERATED from `wt hook --help-page` -->
