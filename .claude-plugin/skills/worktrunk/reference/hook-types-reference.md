# Hook Types Reference

Detailed behavior and use cases for all five Worktrunk hook types.

## Hook Type Comparison

| Hook | When | Blocking? | Fail-Fast? | Variables | Execution |
|------|------|-----------|------------|-----------|-----------|
| `post-create-command` | After creating worktree | Yes | No | Basic | Sequential |
| `post-start-command` | When switching to worktree | No | No | Basic | Parallel |
| `pre-commit-command` | Before committing during merge | Yes | Yes | Basic + Merge | Sequential |
| `pre-merge-command` | Before merging to target | Yes | Yes | Basic + Merge | Sequential |
| `post-merge-command` | After successful merge | Yes | No | Basic + Merge | Sequential |

**Basic variables**: `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`
**Merge variables**: Basic + `{{ target }}`

## Detailed Behavior

### post-create-command

**When it runs**: After creating a new worktree, before switching to it.

**Behavior**:
- Blocks until all commands complete
- User cannot use worktree until complete
- Failure shows error but doesn't abort (worktree still created)
- Commands run sequentially (even if using array format)

**Use cases**:
- Installing dependencies (npm install, cargo build, poetry install)
- Setting up databases (migrations, seeding)
- Copying required files
- Any setup that must complete before work can begin

**Example**:
```toml
post-create-command = [
    "npm install",
    "npm run db:migrate",
    "cp .env.example .env"
]
```

**What happens**: User runs `wt switch --create feature-x`. Commands execute sequentially. User sees progress. After all complete, they're switched to the new worktree.

### post-start-command

**When it runs**: After switching to an existing worktree (not on creation).

**Behavior**:
- Runs in background, doesn't block user
- Multiple commands run in parallel
- Output logged to `.wt-logs/`
- Failure doesn't affect user session

**Use cases**:
- Long builds that can run in background
- Cache warming
- Background sync/pull operations
- Anything slow that doesn't need to block work

**Example**:
```toml
post-start-command = [
    "npm run build",
    "docker-compose up -d",
    "git pull origin main"
]
```

**What happens**: User runs `wt switch feature-x`. All three commands start immediately in parallel. User can work while they run. Check `.wt-logs/` for output.

### pre-commit-command

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
pre-commit-command = [
    "npm run lint",
    "npm run typecheck",
    "npm run format:check"
]
```

**What happens**: User runs `wt merge`. Before creating commit, all three commands run. If any fails, commit is aborted. User fixes issues and tries again.

### pre-merge-command

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
pre-merge-command = [
    "npm test",
    "npm run build"
]
```

**What happens**: User runs `wt merge`. After commit succeeds, before merging, both commands run. If any fails, merge is aborted but commit remains.

### post-merge-command

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
post-merge-command = [
    "npm run deploy",
    "./scripts/notify-slack.sh"
]
```

**What happens**: User runs `wt merge`. After merge succeeds and push completes, commands run in main worktree. Then cleanup happens (branch deletion, worktree removal).

## Execution Order During Merge

Full sequence when running `wt merge`:

1. Validate working tree is clean
2. **Run `pre-commit-command`** (fail-fast)
3. Create commit
4. Switch to main worktree
5. Pull latest changes
6. **Run `pre-merge-command`** (fail-fast)
7. Merge branch into target
8. Push to remote
9. **Run `post-merge-command`** (best-effort)
10. Clean up (delete branch, remove worktree)

## Format Variants

All hooks support three formats:

### Single Command (String)
```toml
post-create-command = "npm install"
```

### Multiple Commands (Array)
```toml
post-create-command = [
    "npm install",
    "npm run build"
]
```

Behavior:
- `post-create-command`: Sequential execution
- `post-start-command`: Parallel execution
- `pre-commit-command`: Sequential execution
- `pre-merge-command`: Sequential execution
- `post-merge-command`: Sequential execution

### Named Commands (Table)
```toml
[post-create-command]
dependencies = "npm install"
database = "npm run db:migrate"
services = "docker-compose up -d"
```

Behavior same as array format, but with descriptive names.

## Template Variables

### Basic Variables (All Hooks)

```toml
post-create-command = "echo 'Working on {{ branch }} in {{ repo }}'"
```

Available:
- `{{ repo }}` - Repository name (e.g., "my-project")
- `{{ branch }}` - Branch name (e.g., "feature-auth")
- `{{ worktree }}` - Absolute path to worktree
- `{{ repo_root }}` - Absolute path to repository root

### Merge Variables (Merge Hooks Only)

```toml
pre-merge-command = "echo 'Merging {{ branch }} into {{ target }}'"
```

Available in: `pre-commit-command`, `pre-merge-command`, `post-merge-command`

Additional variable:
- `{{ target }}` - Target branch for merge (e.g., "main")

### Conditional Logic

Use shell conditionals with variables:

```toml
pre-merge-command = """
if [ "{{ target }}" = "main" ]; then
    npm run test:full
elif [ "{{ target }}" = "staging" ]; then
    npm run test:integration
else
    npm run test:unit
fi
"""
```

## Common Patterns

### Fast Dependencies + Slow Build
```toml
# Blocking: must complete before work starts
post-create-command = "npm install"

# Background: builds while user works
post-start-command = "npm run build"
```

### Progressive Validation
```toml
# Quick checks before commit
pre-commit-command = ["npm run lint", "npm run typecheck"]

# Thorough validation before merge
pre-merge-command = ["npm test", "npm run build"]
```

### Target-Specific Behavior
```toml
post-merge-command = """
if [ "{{ target }}" = "main" ]; then
    npm run deploy:production
elif [ "{{ target }}" = "staging" ]; then
    npm run deploy:staging
fi
"""
```

### Monorepo with Multiple Tools
```toml
[post-create-command]
frontend = "cd frontend && npm install"
backend = "cd backend && cargo build"
database = "docker-compose up -d postgres"

[pre-merge-command]
frontend-tests = "cd frontend && npm test"
backend-tests = "cd backend && cargo test"
```
