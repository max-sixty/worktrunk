# Project Config Reference

Detailed guidance for configuring project-specific Worktrunk hooks at `.config/wt.toml`.

## Guiding Principle: Proactive and Validated

Unlike user config, project config can be created directly since:
- Changes are versioned in git (easily reversible)
- Benefits the entire team
- Standard practice for dev tooling

Always validate commands exist before adding them to config.

## New Project

When users say "set up some hooks for me", follow this discovery process:

### Step 1: Detect Project Type

Check for package manifests:
```bash
ls package.json Cargo.toml pyproject.toml pom.xml go.mod
```

### Step 2: Identify Available Commands

<example type="detecting-npm-scripts">

For npm projects, read `package.json`:
```bash
cat package.json | grep -A 20 '"scripts"'
```

Look for: `lint`, `test`, `typecheck`, `build`, `format`

</example>

<example type="detecting-cargo-commands">

For Rust projects, common commands:
- `cargo build`
- `cargo test`
- `cargo clippy`
- `cargo fmt --check`

</example>

### Step 3: Design Appropriate Hooks

Match hooks to project needs using this decision tree:

- **Dependency installation** (fast, must complete) → `post-create`
- **Tests/linting** (fast, must pass) → `pre-commit` or `pre-merge`
- **Long builds** (slow, optional) → `post-start`
- **Deployment** (after merge) → `post-merge`

### Step 4: Validate Commands Work

Before adding to config, check:
```bash
npm run lint    # Check script exists
which cargo     # Check tool exists
```

### Step 5: Create `.config/wt.toml`

<example type="npm-project-config">

Typical npm project:
```toml
# Install dependencies when creating new worktrees (blocking)
post-create = "npm install"

# Validate code quality before committing (blocking, fail-fast)
[pre-commit]
lint = "npm run lint"
typecheck = "npm run typecheck"

# Run tests before merging (blocking, fail-fast)
pre-merge = "npm test"
```

</example>

<example type="rust-project-config">

Typical Rust project:
```toml
# Build runs in background (slow)
post-start = "cargo build"

# Format and lint before committing (blocking, fail-fast)
[pre-commit]
format = "cargo fmt --check"
lint = "cargo clippy -- -D warnings"

# Run tests before merging (blocking, fail-fast)
pre-merge = "cargo test"
```

</example>

### Step 6: Add Comments Explaining Choices

Document why each hook exists:
```toml
# Dependencies must be installed before worktree is usable
post-create = "npm install"

# Enforce code quality standards (matches CI checks)
[pre-commit]
lint = "npm run lint"
typecheck = "npm run typecheck"
```

### Step 7: Suggest Testing

```bash
# Create a test worktree to verify hooks work
wt switch --create test-hooks
```

## Add Hook

When users want to add automation to an existing project:

### Step 1: Read Existing Config

```bash
cat .config/wt.toml
```

### Step 2: Determine Appropriate Hook Type

Ask: When should this run?
- Creating worktree (blocking) → `post-create`
- Creating worktree (background) → `post-start`
- Every switch operation → `post-switch`
- Before committing → `pre-commit`
- Before merging → `pre-merge`
- After merging → `post-merge`
- Before worktree removal → `pre-remove`

### Step 3: Handle Format Conversion if Needed

<example type="adding-to-single-command">

Current (single command):
```toml
post-create = "npm install"
```

Adding "npm run db:migrate" — convert to named table:
```toml
[post-create]
install = "npm install"
migrate = "npm run db:migrate"
```

</example>

<example type="adding-to-table">

Current (named table):
```toml
[pre-commit]
lint = "npm run lint"
```

Adding typecheck — just add another entry:
```toml
[pre-commit]
lint = "npm run lint"
typecheck = "npm run typecheck"
```

</example>

### Step 4: Update the File

Preserve existing structure and comments.

## Variables

All hooks support template variables for dynamic behavior.

### Basic Variables (All Hooks)

Available in all hook types:
- `{{ repo }}` - Repository name (e.g., "my-project")
- `{{ repo_path }}` - Absolute path to repository (e.g., "/path/to/my-project")
- `{{ branch }}` - Raw branch name (e.g., "feature/auth")
- `{{ worktree_name }}` - Worktree directory name (e.g., "my-project.feature-auth")
- `{{ worktree_path }}` - Absolute path to worktree (e.g., "/path/to/my-project.feature-auth")
- `{{ default_branch }}` - Default branch name (e.g., "main")
- `{{ commit }}` - Full HEAD commit SHA
- `{{ short_commit }}` - Short HEAD commit SHA (7 chars)
- `{{ remote }}` - Primary remote name (e.g., "origin")
- `{{ remote_url }}` - Remote URL (e.g., "git@github.com:user/repo.git")
- `{{ upstream }}` - Upstream tracking branch (e.g., "origin/feature")

### Filters

- `{{ branch | sanitize }}` - Replace `/` and `\` with `-` (e.g., "feature-auth")
- `{{ branch | hash_port }}` - Hash string to deterministic port (10000-19999)

Example:
```toml
[post-start]
dev = "npm run dev --port {{ branch | hash_port }}"
cache = "ln -sf {{ repo_path }}/node_modules.{{ branch | sanitize }} node_modules"
```

<example type="basic-variables">

```toml
post-create = "echo 'Working on {{ branch }} in {{ repo }}'"
```

</example>

### Merge Variables (Merge Hooks Only)

Available in: `pre-commit`, `pre-merge`, `post-merge`

Additional variable:
- `{{ target }}` - Target branch for merge (e.g., "main")

<example type="conditional-with-variables">

Run different tests based on target branch:
```toml
pre-merge = """
if [ "{{ target }}" = "main" ]; then
    npm run test:full
else
    npm run test:quick
fi
"""
```

</example>

## Formats

All hooks support two command formats.

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
- `post-create`: Sequential
- `post-start`: Parallel
- `post-switch`: Parallel
- `pre-commit`: Sequential
- `pre-merge`: Sequential
- `post-merge`: Sequential
- `pre-remove`: Sequential

Named commands appear in output with their labels, which helps identify which command succeeded or failed.

## Hook Types

Seven hook types with different timing and behavior:

### post-create

**When**: After creating new worktree (blocking, before user switches)
**Blocking**: Yes (user waits)
**Fail-fast**: No (shows error but continues)
**Execution**: Sequential

**Use for**:
- Installing dependencies (npm install, cargo build)
- Database migrations
- Any setup that must complete before work begins

<example type="post-create">

```toml
[post-create]
install = "npm install"
migrate = "npm run db:migrate"
```

</example>

### post-start

**When**: After creating new worktree (background, after user switches)
**Blocking**: No (runs in background)
**Fail-fast**: No
**Execution**: Parallel

**Use for**:
- Long builds
- Cache warming
- Background sync

<example type="post-start">

```toml
[post-start]
build = "npm run build"
services = "docker-compose up -d"
```

</example>

### post-switch

**When**: After every switch operation (background)
**Blocking**: No (runs in background)
**Fail-fast**: No
**Execution**: Parallel

**Use for**:
- Renaming terminal tabs
- Updating tmux window names
- IDE notifications

<example type="post-switch">

```toml
post-switch = "echo 'Switched to {{ branch }}'"
```

</example>

### pre-commit

**When**: Before committing during merge
**Blocking**: Yes
**Fail-fast**: Yes (any failure aborts commit)
**Execution**: Sequential

**Use for**:
- Linting
- Formatting checks
- Type checking

<example type="pre-commit">

```toml
[pre-commit]
lint = "npm run lint"
typecheck = "npm run typecheck"
```

</example>

### pre-merge

**When**: Before merging to target branch
**Blocking**: Yes
**Fail-fast**: Yes (any failure aborts merge)
**Execution**: Sequential

**Use for**:
- Running tests
- Build verification
- Security scans

<example type="pre-merge">

```toml
pre-merge = "npm test"
```

</example>

### post-merge

**When**: After successful merge, before cleanup
**Blocking**: Yes
**Fail-fast**: No (merge already complete)
**Execution**: Sequential

**Use for**:
- Deployment
- Notifications
- Cache invalidation

<example type="post-merge">

```toml
post-merge = "npm run deploy"
```

</example>

### pre-remove

**When**: Before worktree removal during `wt remove`
**Blocking**: Yes
**Fail-fast**: Yes (any failure aborts removal)
**Execution**: Sequential

**Use for**:
- Cleanup tasks (temp files, caches)
- Saving state
- Notifying external systems
- Stopping services

<example type="pre-remove">

```toml
[pre-remove]
cleanup = "rm -rf /tmp/cache/{{ branch }}"
```

</example>

See `hook-types-reference.md` for complete behavioral details.

## Validation & Safety

### Before Adding Commands

Check commands are safe and exist:

<example type="validation-checks">

```bash
# Verify command exists
which npm
which cargo

# For npm, verify script exists
npm run lint --dry-run

# For shell commands, check syntax
bash -n -c "if [ true ]; then echo ok; fi"
```

</example>

### Dangerous Patterns

Warn before creating hooks with:
- Destructive commands: `rm -rf`, `DROP TABLE`
- External dependencies: `curl http://...`
- Privilege escalation: `sudo`

Reject obviously dangerous commands:
- `rm -rf /`
- Fork bombs
- Arbitrary code execution

## Troubleshooting

### Hook Not Running

Check sequence:
1. Verify `.config/wt.toml` exists: `ls -la .config/wt.toml`
2. Check TOML syntax: `cat .config/wt.toml`
3. Verify hook name spelling matches one of the seven types
4. Test command manually in terminal

### Hook Failing

Debug steps:
1. Run command manually in worktree
2. Check for missing dependencies (npm packages, system tools)
3. Verify template variables expand correctly
4. For background hooks, check `.git/wt-logs/` for output

### Slow Blocking Hooks

Move long-running commands to background:

<example type="blocking-to-background">

Before (blocks for minutes):
```toml
post-create = "npm run build"
```

After (runs in background):
```toml
post-create = "npm install"  # Fast, blocking
post-start = "npm run build"  # Slow, background
```

</example>

## Key Commands

```bash
wt config list                    # View project config
cat .config/wt.toml               # Read config directly
wt switch --create test-hooks     # Test hooks work
```

## Dev Server URL

Add a URL column to `wt list` showing dev server links per worktree:

```toml
[list]
url = "http://localhost:{{ branch | hash_port }}"
```

URLs are dimmed when the port isn't listening. The template supports all variables plus filters.

Example with subdomain:
```toml
[list]
url = "http://{{ branch }}.lvh.me:3000"
```

## Config File Location

- **Always at**: `<repo>/.config/wt.toml` (checked into git)
- **Background logs**: `.git/wt-logs/` (in git directory, not tracked)

## Example Config

See `dev/wt.example.toml` in the worktrunk repository for a complete annotated example.
