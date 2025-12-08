+++
title = "wt hook"
weight = 50

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt hook --help-page` — edit src/cli.rs to update -->

Run project-defined lifecycle hooks from `.config/wt.toml`.

Hooks are commands that run automatically during worktree operations (`wt switch --create`, `wt merge`, `wt remove`). Use `wt hook` to run them manually for testing or CI.

## Examples

Run pre-merge hooks (for testing):

```bash
wt hook pre-merge
```

Run in CI (skip approval prompts):

```bash
wt hook pre-merge --force
```

## Hook types

| Hook | When | Blocking | Fail-fast | Execution |
|------|------|----------|-----------|-----------|
| `post-create` | After worktree created | Yes | No | Sequential |
| `post-start` | After worktree created | No | No | Parallel (background) |
| `pre-commit` | Before commit during merge | Yes | Yes | Sequential |
| `pre-merge` | Before merging to target | Yes | Yes | Sequential |
| `post-merge` | After successful merge | Yes | No | Sequential |
| `pre-remove` | Before worktree removed | Yes | Yes | Sequential |

**Blocking**: Command waits for hook to complete before continuing.
**Fail-fast**: First failure aborts the operation.

## Configuration formats

Hooks can be a single command or multiple named commands in `.config/wt.toml`:

**Single command (string):**

```toml
post-create = "npm install"
```

**Multiple commands (table):**

```toml
[post-create]
install = "npm install"
build = "npm run build"
```

Named commands run sequentially in declaration order and appear in output with their labels.

## Template variables

Hooks can use template variables that expand at runtime.

**All hooks:**

| Variable | Example | Description |
|----------|---------|-------------|
| `{{ repo }}` | my-project | Repository name |
| `{{ branch }}` | feature-foo | Branch name |
| `{{ worktree }}` | /path/to/worktree | Absolute worktree path |
| `{{ worktree_name }}` | my-project.feature-foo | Worktree directory name |
| `{{ repo_root }}` | /path/to/main | Repository root path |
| `{{ default_branch }}` | main | Default branch name |
| `{{ commit }}` | a1b2c3d4e5f6... | Full HEAD commit SHA |
| `{{ short_commit }}` | a1b2c3d | Short HEAD commit SHA |
| `{{ remote }}` | origin | Primary remote name |
| `{{ upstream }}` | origin/feature | Upstream tracking branch |

**Merge hooks** (`pre-commit`, `pre-merge`, `post-merge`):

| Variable | Example | Description |
|----------|---------|-------------|
| `{{ target }}` | main | Target branch for merge |

## Hook details

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

Output logged to `.git/wt-logs/{branch}-post-start-{name}.log`.

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

Runs after successful merge in the **main worktree**, **best-effort**. Failures are logged but don't abort.

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

## When hooks run during merge

- **pre-commit** — After staging, before squash commit
- **pre-merge** — After rebase, before merge to target
- **pre-remove** — Before removing worktree during cleanup
- **post-merge** — After cleanup completes

See [wt merge](@/merge.md#pipeline) for the complete pipeline.

## Security & approval

Project commands require approval on first run:

```
🟡 repo needs approval to execute 3 commands:

⚪ post-create install:
   echo 'Installing dependencies...'

❓ Allow and remember? [y/N]
```

- Approvals are saved to user config (`~/.config/worktrunk/config.toml`)
- If a command changes, new approval is required
- Use `--force` to bypass prompts (useful for CI/automation)

Manage approvals with `wt config approvals add` and `wt config approvals clear`.

## Skipping hooks

Use `--no-verify` to skip all project hooks:

```bash
wt switch --create temp --no-verify    # Skip post-create and post-start
wt merge --no-verify                   # Skip pre-commit, pre-merge, post-merge
wt remove feature --no-verify          # Skip pre-remove
```

## Logging

Background operations log to `.git/wt-logs/` in the main worktree:

| Operation | Log file |
|-----------|----------|
| post-start | `{branch}-post-start-{name}.log` |
| Background removal | `{branch}-remove.log` |

## Example configurations

### Node.js / TypeScript

```toml
[post-create]
install = "npm ci"

[post-start]
dev = "npm run dev"

[pre-commit]
lint = "npm run lint"
typecheck = "npm run typecheck"

[pre-merge]
test = "npm test"
build = "npm run build"
```

### Rust

```toml
[post-create]
build = "cargo build"

[pre-commit]
format = "cargo fmt -- --check"
clippy = "cargo clippy -- -D warnings"

[pre-merge]
test = "cargo test"
build = "cargo build --release"

[post-merge]
install = "cargo install --path ."
```

### Python (uv)

```toml
[post-create]
install = "uv sync"

[pre-commit]
format = "uv run ruff format --check ."
lint = "uv run ruff check ."

[pre-merge]
test = "uv run pytest"
typecheck = "uv run mypy ."
```

### Monorepo

```toml
[post-create]
frontend = "cd frontend && npm ci"
backend = "cd backend && cargo build"

[post-start]
database = "docker-compose up -d postgres"

[pre-merge]
frontend-tests = "cd frontend && npm test"
backend-tests = "cd backend && cargo test"
```

## Common patterns

### Fast dependencies + slow build

Install dependencies blocking, build in background:

```toml
post-create = "npm install"
post-start = "npm run build"
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

Different behavior based on merge target:

```toml
post-merge = """
if [ "{{ target }}" = "main" ]; then
    npm run deploy:production
elif [ "{{ target }}" = "staging" ]; then
    npm run deploy:staging
fi
"""
```

### Symlinks and caches

Set up shared resources. The `{{ repo_root }}` variable points to the main worktree:

```toml
[post-create]
cache = "ln -sf {{ repo_root }}/node_modules node_modules"
env = "cp {{ repo_root }}/.env.local .env"
```

## See also

- [wt merge](@/merge.md) — Runs hooks automatically during merge
- [wt switch](@/switch.md) — Runs post-create/post-start hooks on `--create`
- [wt config](@/config.md) — Manage hook approvals

---

## Command reference

<!-- ⚠️ AUTO-GENERATED from `wt hook --help-page` — edit cli.rs to update -->

```
wt hook - Run project hooks
Usage: wt hook [OPTIONS] <COMMAND>

Commands:
  post-create  Run post-create hooks
  post-start   Run post-start hooks
  pre-commit   Run pre-commit hooks
  pre-merge    Run pre-merge hooks
  post-merge   Run post-merge hooks
  pre-remove   Run pre-remove hooks

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
