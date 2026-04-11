# Extending Worktrunk

Worktrunk has three extension mechanisms.

**[Hooks](#hooks)** run shell commands at lifecycle events — creating a worktree, merging, removing. They're configured in TOML and run automatically.

**[Aliases](#aliases)** define reusable commands invoked via `wt step <name>`. Same template variables as hooks, but triggered manually.

**[External subcommands](#external-subcommands)** are standalone executables. Drop `wt-foo` on `PATH` and it becomes `wt foo`. No configuration needed.

| | Hooks | Aliases | External subcommands |
|---|---|---|---|
| **Trigger** | Automatic (lifecycle events) | Manual (`wt step <name>`) | Manual (`wt <name>`) |
| **Defined in** | TOML config | TOML config | Any executable on `PATH` |
| **Template variables** | Yes | Yes | No |
| **Shareable via repo** | `.config/wt.toml` | `.config/wt.toml` | Distribute the binary |
| **Language** | Shell commands | Shell commands | Any |

## Hooks

Hooks are shell commands that run at key points in the worktree lifecycle. Ten hooks cover five events:

| Event | `pre-` (blocking) | `post-` (background) |
|-------|-------------------|---------------------|
| **switch** | `pre-switch` | `post-switch` |
| **start** | `pre-start` | `post-start` |
| **commit** | `pre-commit` | `post-commit` |
| **merge** | `pre-merge` | `post-merge` |
| **remove** | `pre-remove` | `post-remove` |

`pre-*` hooks block — failure aborts the operation. `post-*` hooks run in the background.

### Configuration

Hooks live in two places:

- **User config** (`~/.config/worktrunk/config.toml`) — personal, applies everywhere, trusted
- **Project config** (`.config/wt.toml`) — shared with the team, requires [approval](https://worktrunk.dev/hook/#wt-hook-approvals) on first run

Three formats, from simplest to most expressive:

```toml
# Single command
pre-start = "npm ci"
```

```toml
# Named commands (concurrent for post-*, serial for pre-*)
[post-start]
server = "npm start"
watcher = "npm run watch"
```

```toml
# Pipeline: steps run in order, commands within a step run concurrently
post-start = [
    "npm ci",
    { server = "npm start", build = "npm run build" }
]
```

### Template variables

Hook commands are templates. Variables expand at execution time:

```toml
[post-start]
server = "npm run dev -- --port {{ branch | hash_port }}"
env = "echo 'PORT={{ branch | hash_port }}' > .env.local"
```

Core variables include `branch`, `worktree_path`, `commit`, `repo`, `default_branch`, and context-dependent ones like `target` during merge. Filters like `sanitize`, `hash_port`, and `sanitize_db` transform values for specific uses.

See [`wt hook`](https://worktrunk.dev/hook/#template-variables) for the full variable and filter reference.

### Common patterns

```toml
# .config/wt.toml

# Install dependencies when creating a worktree
[pre-start]
deps = "npm ci"

# Run tests before merging
[pre-merge]
test = "npm test"
lint = "npm run lint"

# Dev server per worktree on a deterministic port
[post-start]
server = "npm run dev -- --port {{ branch | hash_port }}"
```

See [Tips & Patterns](https://worktrunk.dev/tips-patterns/) for more recipes: dev server per worktree, database per worktree, tmux sessions, Caddy subdomain routing.

## Aliases

Aliases are custom commands invoked via `wt step <name>`. They share the same template variables and approval model as hooks.

```toml
# .config/wt.toml
[aliases]
deploy = "make deploy BRANCH={{ branch }}"
open = "open http://localhost:{{ branch | hash_port }}"
```

```bash
$ wt step deploy
$ wt step deploy --dry-run
$ wt step deploy --var env=staging
```

When both user and project config define the same alias name, both run — user first, then project. Project-config aliases require approval, same as project hooks.

Alias names that collide with built-in step commands (`commit`, `squash`, `rebase`, etc.) are shadowed by the built-in.

See [`wt step` — Aliases](https://worktrunk.dev/step/#aliases) for the full reference.

### Common patterns

Three aliases for a recurring case — changes accumulated on `main` that belong on a feature branch. Each composes `wt switch --create` with `--execute`, so the inner command runs in the new worktree and shell integration carries both the `cd` and the `--execute` step back to the parent shell.

```toml
# .config/wt.toml
[aliases]
# Move all in-progress changes (staged + unstaged + untracked) to a new
# worktree. Source becomes clean.
#   wt step move-changes --var to=feature-xyz
move-changes = '''if git diff --quiet HEAD && test -z "$(git ls-files --others --exclude-standard)"; then wt switch --create {{ to }}; else git stash push --include-untracked --quiet && wt switch --create {{ to }} --execute='git stash pop --index'; fi'''

# Copy all changes (staged + unstaged + untracked). Source is unchanged.
#   wt step copy-changes --var to=feature-xyz
copy-changes = '''if git diff --quiet HEAD && test -z "$(git ls-files --others --exclude-standard)"; then wt switch --create {{ to }}; else git stash push --include-untracked --quiet && git stash apply --index --quiet && wt switch --create {{ to }} --execute='git stash pop --index'; fi'''

# Copy only staged changes. Source is unchanged.
#   wt step copy-staged --var to=feature-xyz
copy-staged = '''if git diff --cached --quiet; then wt switch --create {{ to }}; else p=$(mktemp) && git diff --cached > "$p" && wt switch --create {{ to }} --execute="git apply --index '$p' && rm '$p'"; fi'''
```

`--index` preserves the staged-vs-unstaged split when popping. The `git diff --quiet HEAD` guard skips the stash dance when the source is already clean, avoiding noise and never touching a pre-existing stash. If `--base` differs from `HEAD`, `git apply` and `git stash pop` may reject hunks that don't match — the same constraint a native flag would face.

## External subcommands

[experimental]

Any executable named `wt-<name>` on `PATH` becomes available as `wt <name>` — the same pattern git uses for `git-foo`. Built-in commands always take precedence.

```bash
$ wt sync origin              # runs: wt-sync origin
$ wt -C /tmp/repo sync        # -C is forwarded as the child's working directory
```

Arguments pass through verbatim, stdio is inherited, and the child's exit code propagates unchanged. External subcommands don't have access to template variables.

If nothing matches — no built-in, no nested subcommand, no `wt-<name>` on `PATH` — wt prints a "not a wt command" error with a typo suggestion.
