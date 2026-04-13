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
# Pipeline: blocks run in order, commands within a block run concurrently
[[post-start]]
install = "npm ci"

[[post-start]]
server = "npm start"
build = "npm run build"
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
[aliases]
deploy = "make deploy BRANCH={{ branch }}"
open = "open http://localhost:{{ branch | hash_port }}"
```

```bash
$ wt step deploy
$ wt step deploy --dry-run
$ wt step deploy --env=staging
```

An `up` alias that fetches all remotes and rebases each worktree onto its upstream:

```toml
[aliases]
up = '''
git fetch --all --prune && wt step for-each -- '
  git rev-parse --verify @{u} >/dev/null 2>&1 || exit 0
  g=$(git rev-parse --git-dir)
  test -d "$g/rebase-merge" -o -d "$g/rebase-apply" && exit 0
  git rebase @{u} --no-autostash || git rebase --abort
''''
```

When both user and project config define the same alias name, both run — user first, then project. Project-config aliases require approval, same as project hooks.

Alias names that collide with built-in step commands (`commit`, `squash`, `rebase`, etc.) are shadowed by the built-in.

### Recipe: move or copy in-progress changes to a new worktree

Aliases compose existing commands into richer workflows. These three aliases wrap `wt switch --create` with git's stash and diff plumbing so staged, unstaged, and untracked changes can follow you into a new worktree:

```toml
# .config/wt.toml
[aliases]
# Move all in-progress changes (staged + unstaged + untracked) to a new
# worktree. Source becomes clean.
#   wt step move-changes --to=feature-xyz
move-changes = '''if git diff --quiet HEAD && test -z "$(git ls-files --others --exclude-standard)"; then wt switch --create {{ to }}; else git stash push --include-untracked --quiet && wt switch --create {{ to }} --execute='git stash pop --index'; fi'''

# Copy all changes (staged + unstaged + untracked) to a new worktree.
# Source is unchanged.
#   wt step copy-changes --to=feature-xyz
copy-changes = '''if git diff --quiet HEAD && test -z "$(git ls-files --others --exclude-standard)"; then wt switch --create {{ to }}; else git stash push --include-untracked --quiet && git stash apply --index --quiet && wt switch --create {{ to }} --execute='git stash pop --index'; fi'''

# Copy only staged changes to a new worktree. Source is unchanged.
#   wt step copy-staged --to=feature-xyz
copy-staged = '''if git diff --cached --quiet; then wt switch --create {{ to }}; else p=$(mktemp) && git diff --cached > "$p" && wt switch --create {{ to }} --execute="git apply --index '$p' && rm '$p'"; fi'''
```

How they work:

- **`move-changes`** stashes everything (`--include-untracked`), creates the new worktree, then runs `git stash pop --index` inside it via `--execute`. The `--index` flag preserves the staged/unstaged split; the clean-state guard avoids touching a pre-existing stash.
- **`copy-changes`** adds one extra step — `git stash apply --index --quiet` right after the push — to restore the source worktree before the pop happens in the new one. Both worktrees end up with identical in-progress state, untracked files included.
- **`copy-staged`** writes `git diff --cached` to a tempfile and applies it with `git apply --index` in the new worktree. A diff (rather than `git stash --staged`) handles files where staged and unstaged hunks overlap on the same lines.

Because an inner `wt switch --create` inside an alias [propagates its `cd` to the parent shell](https://worktrunk.dev/step/#aliases), all three drop the shell in the new worktree directly.

### Recipe: tail a specific hook log

`wt config state logs --format=json` emits structured entries — `branch`, `source`, `hook_type`, `name`, `path`. Pipe through `jq` to resolve one entry, then wrap in an alias for quick access:

```toml
[aliases]
# Tail the current worktree's post-start hook named {{ name }} (handles sanitization):
#   wt step hook-log --name=feature/auth
hook-log = '''
tail -f "$(wt config state logs --format=json | jq -r --arg name "{{ name | sanitize_hash }}" '
  .hook_output[]
  | select(.branch == "{{ branch | sanitize_hash }}" and .hook_type == "post-start" and .name == $name)
  | .path
' | head -1)"
'''
```

The `sanitize_hash` filter produces a filesystem-safe name with a hash suffix that keeps distinct originals unique — the same transformation Worktrunk applies on disk — so the alias resolves the right log even for branch and hook names containing characters like `/`.

See [`wt step` — Aliases](https://worktrunk.dev/step/#aliases) for the full reference.

## External subcommands

[experimental]

Any executable named `wt-<name>` on `PATH` becomes available as `wt <name>` — the same pattern git uses for `git-foo`. Built-in commands always take precedence.

```bash
$ wt sync origin              # runs: wt-sync origin
$ wt -C /tmp/repo sync        # -C is forwarded as the child's working directory
```

Arguments pass through verbatim, stdio is inherited, and the child's exit code propagates unchanged. External subcommands don't have access to template variables.

If nothing matches — no built-in, no nested subcommand, no `wt-<name>` on `PATH` — wt prints a "not a wt command" error with a typo suggestion.
