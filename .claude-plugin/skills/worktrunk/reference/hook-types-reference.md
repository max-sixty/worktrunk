# Hook Types Reference

Detailed behavior and use cases for all seven Worktrunk hook types.

## Hook Type Comparison

| Hook | When | Blocking? | Fail-Fast? | Variables | Execution |
|------|------|-----------|------------|-----------|-----------|
| `post-create` | After creating worktree | Yes | No | Basic + Base | Sequential |
| `post-start` | After creating worktree | No (background) | No | Basic + Base | Parallel |
| `post-switch` | After every switch | No (background) | No | Basic + Base | Parallel |
| `pre-commit` | Before committing during merge | Yes | Yes | Basic + Merge | Sequential |
| `pre-merge` | Before merging to target | Yes | Yes | Basic + Merge | Sequential |
| `post-merge` | After successful merge | Yes | No | Basic + Merge | Sequential |
| `pre-remove` | Before worktree removed | Yes | Yes | Basic | Sequential |

**Basic variables**: `{{ repo }}`, `{{ repo_path }}`, `{{ branch }}`, `{{ worktree_name }}`, `{{ worktree_path }}`, `{{ main_worktree_path }}`, `{{ default_branch }}`, `{{ commit }}`, `{{ short_commit }}`, `{{ remote }}`, `{{ remote_url }}`, `{{ upstream }}`
**Base variables**: `{{ base }}`, `{{ base_worktree_path }}`
**Merge variables**: `{{ target }}`
**Filters**: `{{ branch | sanitize }}` (replace `/` `\` with `-`), `{{ branch | hash_port }}` (port 10000-19999)

## Detailed Behavior

### post-create

**When it runs**: After creating a new worktree, before switching to it.

**Behavior**:
- Blocks until all commands complete
- User cannot use worktree until complete
- Failure shows error but doesn't abort (worktree still created)
- Commands run sequentially

**Use cases**:
- Installing dependencies (npm install, cargo build, poetry install)
- Setting up databases (migrations, seeding)
- Copying required files
- Any setup that must complete before work can begin

**Example**:
```toml
[post-create]
install = "npm install"
migrate = "npm run db:migrate"
env = "cp .env.example .env"
```

**What happens**: User runs `wt switch --create feature-x`. Commands execute sequentially. User sees progress. After all complete, they're switched to the new worktree.

### post-start

**When it runs**: After creating a new worktree (not when switching to existing).

**Behavior**:
- Runs in background, doesn't block user
- Multiple commands run in parallel
- Output logged to `.git/wt-logs/`
- Failure doesn't affect user session

**Use cases**:
- Long builds that can run in background
- Cache warming
- Background sync/pull operations
- Anything slow that doesn't need to block work

**Example**:
```toml
[post-start]
build = "npm run build"
services = "docker-compose up -d"
sync = "git pull origin main"
```

**What happens**: User runs `wt switch --create feature-x`. After creation completes, all three commands start immediately in parallel in background. User can work while they run. Check `.git/wt-logs/` for output.

### post-switch

**When it runs**: After every switch operation (creating new, switching to existing, or switching to current).

**Behavior**:
- Runs in background, doesn't block user
- Multiple commands run in parallel
- Output logged to `.git/wt-logs/`
- Triggers on all switch results

**Use cases**:
- Renaming terminal tabs
- Updating tmux window names
- IDE notifications
- Any per-switch automation

**Example**:
```toml
post-switch = "echo 'Switched to {{ branch }}'"
```

**What happens**: Every time the user runs `wt switch`, this fires in the background. Unlike post-start, it runs on every switch, not just when creating new worktrees.

### pre-commit

**When it runs**: Before committing changes during `wt merge`.

**Behavior**:
- Blocks until all commands complete
- Commands run sequentially
- ANY failure aborts the commit (fail-fast)
- Exit code 0 required from all commands

**Use cases**:
- Linting (must pass before commit)
- Formatting checks
- Type checking
- Quick validation that must pass

**Example**:
```toml
[pre-commit]
lint = "npm run lint"
typecheck = "npm run typecheck"
format = "npm run format:check"
```

**What happens**: User runs `wt merge`. Before creating commit, all three commands run. If any fails, commit is aborted. User fixes issues and tries again.

### pre-merge

**When it runs**: Before merging to target branch during `wt merge`.

**Behavior**:
- Blocks until all commands complete
- Commands run sequentially
- ANY failure aborts the merge (fail-fast)
- Exit code 0 required from all commands
- Runs after commit succeeds

**Use cases**:
- Running tests (must pass before merge)
- Security scans
- Build verification
- Any validation that must pass before merge

**Example**:
```toml
[pre-merge]
test = "npm test"
build = "npm run build"
```

**What happens**: User runs `wt merge`. After commit succeeds, before merging, both commands run. If any fails, merge is aborted but commit remains.

### post-merge

**When it runs**: After successful merge to target branch, before cleanup.

**Behavior**:
- Blocks until all commands complete
- Commands run sequentially
- Runs in main worktree, not feature branch worktree
- Failure shows error but doesn't abort (merge already happened)

**Use cases**:
- Deployment (after merge to main)
- Notifications (Slack, email)
- Cache invalidation
- Triggering CI/CD
- Any post-merge automation

**Example**:
```toml
[post-merge]
deploy = "npm run deploy"
notify = "./scripts/notify-slack.sh"
```

**What happens**: User runs `wt merge`. After merge succeeds and push completes, commands run in main worktree. Then cleanup happens (branch deletion, worktree removal).

### pre-remove

**When it runs**: Before worktree removal during `wt remove`.

**Behavior**:
- Blocks until all commands complete
- Commands run sequentially
- ANY failure aborts the removal (fail-fast)
- Exit code 0 required from all commands

**Use cases**:
- Cleanup tasks (temp files, caches)
- Saving state before removal
- Notifying external systems
- Stopping services

**Example**:
```toml
[pre-remove]
cleanup = "rm -rf /tmp/cache/{{ branch }}"
stop = "docker-compose down"
```

**What happens**: User runs `wt remove`. Before the worktree is removed, commands run. If any fails, removal is aborted.

## Execution Order During Merge

Full sequence when running `wt merge`:

1. Validate working tree is clean
2. **Run `pre-commit`** (fail-fast)
3. Create commit
4. Switch to main worktree
5. Pull latest changes
6. **Run `pre-merge`** (fail-fast)
7. Merge branch into target
8. Push to remote
9. **Run `post-merge`** (best-effort)
10. **Run `pre-remove`** (fail-fast, if cleanup enabled)
11. Clean up (delete branch, remove worktree)

## Format Variants

All hooks support two formats:

### Single Command (String)
```toml
post-create = "npm install"
```

### Multiple Commands (Named Table)
```toml
[post-create]
dependencies = "npm install"
database = "npm run db:migrate"
services = "docker-compose up -d"
```

Behavior:
- `post-create`: Sequential execution
- `post-start`: Parallel execution
- `post-switch`: Parallel execution
- `pre-commit`: Sequential execution
- `pre-merge`: Sequential execution
- `post-merge`: Sequential execution
- `pre-remove`: Sequential execution

Named commands appear in output with their labels, making it easier to identify which command succeeded or failed.

## Template Variables

### Basic Variables (All Hooks)

```toml
post-create = "echo 'Working on {{ branch }} in {{ repo }}'"
```

Available:
- `{{ repo }}` - Repository directory name (e.g., "myproject")
- `{{ repo_path }}` - Absolute path to repository (e.g., "/path/to/myproject")
- `{{ branch }}` - Branch name (e.g., "feature/auth")
- `{{ worktree_name }}` - Worktree directory name (e.g., "myproject.feature-auth")
- `{{ worktree_path }}` - Absolute path to worktree (e.g., "/path/to/myproject.feature-auth")
- `{{ main_worktree_path }}` - Absolute path to main worktree (e.g., "/path/to/myproject")
- `{{ default_branch }}` - Default branch name (e.g., "main")
- `{{ commit }}` - Full HEAD commit SHA
- `{{ short_commit }}` - Short HEAD commit SHA (7 chars)
- `{{ remote }}` - Primary remote name (e.g., "origin")
- `{{ remote_url }}` - Remote URL (e.g., "git@github.com:user/repo.git")
- `{{ upstream }}` - Upstream tracking branch (e.g., "origin/feature")

### Base Variables (Creation Hooks)

```toml
post-create = "echo 'Created from {{ base }} at {{ base_worktree_path }}'"
```

Available in: `post-create`, `post-start`, `post-switch` — only when creating a new worktree (not when switching to existing worktrees).

Additional variables:
- `{{ base }}` - Base branch that new worktree was created from (e.g., "main")
- `{{ base_worktree_path }}` - Absolute path to base branch worktree (empty if base has no worktree)

### Merge Variables (Merge Hooks Only)

```toml
pre-merge = "echo 'Merging {{ branch }} into {{ target }}'"
```

Available in: `pre-commit`, `pre-merge`, `post-merge`

Additional variable:
- `{{ target }}` - Target branch for merge (e.g., "main")

### Conditional Logic

Use shell conditionals with variables:

```toml
pre-merge = """
if [ "{{ target }}" = "main" ]; then
    npm run test:full
elif [ "{{ target }}" = "staging" ]; then
    npm run test:integration
else
    npm run test:unit
fi
"""
```

### JSON Context

Hooks also receive context as JSON on stdin, enabling hooks in any language:

```python
import json, sys
ctx = json.load(sys.stdin)
print(f"Setting up {ctx['repo']} on branch {ctx['branch']}")
```

The JSON includes all template variables plus `hook_type` and `hook_name`. This enables repository-specific behavior in user hooks:

```toml
# ~/.config/worktrunk/config.toml
[post-switch]
notify = """
python3 -c '
import json, sys
ctx = json.load(sys.stdin)
if "work-project" in ctx.get("repo", ""):
    print(f"Switched to work project: {ctx[\"branch\"]}")
'
"""
```

## Common Patterns

### Fast Dependencies + Slow Build
```toml
# Blocking: must complete before work starts
post-create = "npm install"

# Background: builds while user works
post-start = "npm run build"
```

### Progressive Validation
```toml
# Quick checks before commit
[pre-commit]
lint = "npm run lint"
typecheck = "npm run typecheck"

# Thorough validation before merge
[pre-merge]
test = "npm test"
build = "npm run build"
```

### Target-Specific Behavior
```toml
post-merge = """
if [ "{{ target }}" = "main" ]; then
    npm run deploy:production
elif [ "{{ target }}" = "staging" ]; then
    npm run deploy:staging
fi
"""
```

### Monorepo with Multiple Tools
```toml
[post-create]
frontend = "cd frontend && npm install"
backend = "cd backend && cargo build"
database = "docker-compose up -d postgres"

[pre-merge]
frontend-tests = "cd frontend && npm test"
backend-tests = "cd backend && cargo test"
```
