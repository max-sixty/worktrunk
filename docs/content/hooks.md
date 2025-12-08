+++
title = "Hooks"
weight = 21

[extra]
group = "Reference"
+++

Hooks automate setup and validation at worktree lifecycle events. They're defined in `.config/wt.toml` (project config) and run automatically during `wt switch --create`, `wt merge`, and `wt remove`.

## Hook types

| Hook | When | Blocking? | Fail-Fast? | Execution |
|------|------|-----------|------------|-----------|
| **post-create** | After worktree created | Yes | No | Sequential |
| **post-start** | After worktree created | No | No | Parallel (background) |
| **pre-commit** | Before commit during merge | Yes | Yes | Sequential |
| **pre-merge** | Before merging to target | Yes | Yes | Sequential |
| **post-merge** | After successful merge | Yes | No | Sequential |
| **pre-remove** | Before worktree removed | Yes | Yes | Sequential |

**Blocking**: Command waits for hook to complete before continuing.
**Fail-fast**: First failure aborts the operation.

## Configuration formats

Hooks can be a single command or multiple named commands. All hooks support both formats in `.config/wt.toml`:

### Single command (string)

```toml
post-create = "npm install"
```

### Multiple commands (table)

```toml
[post-create]
install = "npm install"
build = "npm run build"
```

Named commands appear in output with their labels:

<!-- ⚠️ AUTO-GENERATED-HTML from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_hooks_post_create.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> <span class="cmd">wt switch --create feature-x</span>
🔄 <span class=c>Running post-create <b>install</b>:</span>
<span style='background:var(--bright-white,#fff)'> </span>  <span class=d><span style='color:var(--blue,#00a)'>uv</span></span><span class=d> sync</span>

  Resolved 24 packages in 145ms
  Installed 24 packages in 1.2s
✅ <span class=g>Created new worktree for <b>feature-x</b> from <b>main</b> at <b>../repo.feature-x</b></span>
🔄 <span class=c>Running post-start <b>dev</b>:</span>
<span style='background:var(--bright-white,#fff)'> </span>  <span class=d><span style='color:var(--blue,#00a)'>uv</span></span><span class=d> run dev</span>
{% end %}

<!-- END AUTO-GENERATED -->

## Template variables

Hooks can use template variables that expand at runtime:

### Basic variables (all hooks)

- `{{ repo }}` — Repository name (e.g., "my-project")
- `{{ branch }}` — Branch name (slashes replaced with dashes)
- `{{ worktree }}` — Absolute path to the worktree
- `{{ worktree_name }}` — Worktree directory name (e.g., "my-project.feature-foo")
- `{{ repo_root }}` — Absolute path to the repository root
- `{{ default_branch }}` — Default branch name (e.g., "main")

### Git variables (all hooks)

- `{{ commit }}` — Current HEAD commit SHA (full 40-character hash)
- `{{ short_commit }}` — Current HEAD commit SHA (short 7-character hash)
- `{{ remote }}` — Primary remote name (e.g., "origin")
- `{{ upstream }}` — Upstream tracking branch (e.g., "origin/feature"), if configured

### Merge variables (pre-commit, pre-merge, post-merge)

- `{{ target }}` — Target branch for the merge (e.g., "main")

```toml
# Tag builds with commit hash
post-create = "echo '{{ short_commit }}' > .version"

# Reference merge target
pre-merge = "echo 'Merging {{ branch }} into {{ target }}'"
```

## Hook details

### post-create

Runs after worktree creation, **blocks until complete**. The worktree switch doesn't finish until these commands succeed.

**Use cases**: Installing dependencies, database migrations, copying environment files — anything that must complete before work begins.

```toml
[post-create]
install = "npm ci"
migrate = "npm run db:migrate"
env = "cp .env.example .env"
```

**Behavior**:
- Commands run sequentially in declaration order
- Failure shows error but doesn't abort (worktree already created)
- User cannot work in worktree until complete

### post-start

Runs after worktree creation, **in background**. The worktree switch completes immediately; these run in parallel.

**Use cases**: Long builds, dev servers, file watchers, downloading assets too large for git — anything slow that doesn't need to block.

```toml
[post-start]
build = "npm run build"
server = "npm run dev"
assets = "./scripts/fetch-large-assets"
```

**Behavior**:
- All commands start immediately in parallel
- Output logged to `.git/wt-logs/{branch}-post-start-{name}.log`
- Failures don't affect the user session

### pre-commit

Runs before committing during `wt merge`, **fail-fast**. All commands must exit with code 0 for the commit to proceed.

**Use cases**: Formatters, linters, type checking — quick validation before commit.

```toml
[pre-commit]
format = "cargo fmt -- --check"
lint = "cargo clippy -- -D warnings"
```

**Behavior**:
- Commands run sequentially
- First failure aborts the commit
- Runs for both squash and no-squash merge modes

### pre-merge

Runs before merging to target branch, **fail-fast**. All commands must exit with code 0 for the merge to proceed.

**Use cases**: Tests, security scans, build verification — thorough validation before merge.

```toml
[pre-merge]
test = "cargo test"
build = "cargo build --release"
```

**Behavior**:
- Commands run sequentially
- First failure aborts the merge (commit remains)
- Runs after commit succeeds, before merge

### post-merge

Runs after successful merge and cleanup, **best-effort**. The merge has already completed and the feature worktree has been removed, so failures are logged but don't abort.

**Use cases**: Deployment, notifications, installing updated binaries — post-merge automation.

```toml
post-merge = "cargo install --path ."
```

**Behavior**:
- Commands run sequentially in the **main worktree**
- Runs after cleanup completes
- Failures show errors but don't affect the completed merge

### pre-remove

Runs before worktree removal during `wt remove`, **fail-fast**. All commands must exit with code 0 for the removal to proceed.

**Use cases**: Cleanup tasks, saving state, notifying external systems — validation before removal.

```toml
[pre-remove]
cleanup = "rm -rf /tmp/cache/{{ branch }}"
notify = "echo 'Removing {{ branch }}' >> ~/worktree-log.txt"
```

**Behavior**:
- Commands run sequentially in the **worktree being removed**
- First failure aborts the removal (worktree preserved)
- Runs for both foreground and background removal modes
- Does **not** run for branch-only removal (no worktree)
- Use `--no-verify` to skip

## When hooks run during merge

- **pre-commit** — After staging, before squash commit
- **pre-merge** — After rebase, before merge to target
- **pre-remove** — Before removing worktree during cleanup (failures abort)
- **post-merge** — After cleanup completes

See [wt merge](@/merge.md#pipeline) for the complete pipeline.

## Security & approval

Project commands require approval on first run. When a project defines hooks, the first execution prompts for approval:

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_approval_prompt.snap — edit source to update -->

```
🟡 repo needs approval to execute 3 commands:

⚪ post-create install:
   echo 'Installing dependencies...'

⚪ post-create build:
   echo 'Building project...'

⚪ post-create test:
   echo 'Running tests...'

❓ Allow and remember? [y/N]
```

<!-- END AUTO-GENERATED -->

**Approval behavior**:
- Approvals are saved to user config (`~/.config/worktrunk/config.toml`)
- If a command changes, new approval is required
- Use `--force` to bypass prompts (useful for CI/automation)

Manage approvals with `wt config approvals`:

```bash
wt config approvals list           # Show all approvals
wt config approvals clear <repo>   # Remove approvals for a repo
```

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

Logs overwrite on repeated runs for the same branch/operation. Stale logs from deleted branches persist but are bounded by branch count.

## Running hooks manually

Use `wt step` to run individual hooks:

```bash
wt step post-create    # Run post-create hooks
wt step pre-merge      # Run pre-merge hooks
wt step post-merge     # Run post-merge hooks
wt step pre-remove     # Run pre-remove hooks
```

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

### Python (pip/venv)

```toml
[post-create]
venv = "python -m venv .venv"
install = ".venv/bin/pip install -r requirements.txt"

[pre-merge]
format = ".venv/bin/black --check ."
lint = ".venv/bin/ruff check ."
test = ".venv/bin/pytest"
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
integration = "./scripts/integration-tests.sh"
```

## Common patterns

### Fast dependencies + slow build

Install dependencies blocking (must complete before work), build in background:

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

Set up shared resources that shouldn't be duplicated. The `{{ repo_root }}` variable points to the main worktree:

```toml
[post-create]
cache = "ln -sf {{ repo_root }}/node_modules node_modules"
env = "cp {{ repo_root }}/.env.local .env"
```
