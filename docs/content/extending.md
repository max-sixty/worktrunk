+++
title = "Extending Worktrunk"
description = "Three ways to add custom behavior: hooks for lifecycle automation, aliases for reusable commands, and custom subcommands for standalone tools."
weight = 21

[extra]
group = "Reference"
+++

Worktrunk has three extension mechanisms.

**[Hooks](#hooks)** run shell commands at lifecycle events — creating a worktree, merging, removing. They're configured in TOML and run automatically.

**[Aliases](#aliases)** define reusable commands invoked as `wt <name>`. Same template variables as hooks, but triggered manually.

**[Custom subcommands](#custom-subcommands)** are standalone executables. Drop `wt-foo` on `PATH` and it becomes `wt foo`. No configuration needed.

| | Hooks | Aliases | Custom subcommands |
|---|---|---|---|
| **Trigger** | Automatic (lifecycle events) | Manual (`wt <name>`) | Manual (`wt <name>`) |
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
- **Project config** (`.config/wt.toml`) — shared with the team, requires [approval](@/config.md#wt-config-approvals) on first run

Three formats, from simplest to most expressive.

A single command as a string:

```toml
pre-start = "npm ci"
```

A named table runs commands concurrently for `post-*` hooks and serially for `pre-*`:

```toml
[post-start]
server = "npm start"
watcher = "npm run watch"
```

An array of tables is a pipeline — blocks run in order, commands within a block run concurrently:

```toml
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

See [`wt hook`](@/hook.md#template-variables) for the full variable and filter reference.

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

See [Tips & Patterns](@/tips-patterns.md) for more recipes: dev server per worktree, database per worktree, tmux sessions, Caddy subdomain routing.

## Aliases

`[aliases]` defines commands invoked as `wt <name>`. Templates and approvals behave like hooks.

```toml
[aliases]
deploy = "fly deploy --config=fly.{{ env }}.toml --app=myapp-{{ branch }}"
open = "open http://localhost:{{ branch | hash_port }}"
since-main = "git log --oneline {{ default_branch }}..HEAD"
```

{{ terminal(cmd="wt deploy --env=staging|||wt open") }}

`wt <name>` resolves to a built-in first, then an alias, then a [custom subcommand](#custom-subcommands).

### Passing values

`--KEY=VALUE` (or `--KEY VALUE`) binds `KEY` whenever `{{ KEY }}` appears in the template — `wt deploy --env=staging` sets `{{ env }}` to `staging`. Everything else joins `{{ args }}` (see [Forwarding](#forwarding-positional-arguments)).

Built-in variables can be overridden: `--branch=foo` sets `{{ branch }}` inside the template — the worktree's actual branch doesn't move.

Hyphens in keys become underscores: `--my-var=x` sets `{{ my_var }}`.

### Forwarding positional arguments

`{{ args }}` renders as a space-joined, shell-escaped string — ready to splice into a command:

```toml
[aliases]
s = "wt switch {{ args }}"
```

{{ terminal(cmd="wt s some-branch|||wt s feature/api|||wt s 'has a space'") }}

Index with `{{ args[0] }}`, loop with `{% for a in args %}…{% endfor %}`, count with `{{ args | length }}`. Each element is escaped individually, so `wt run 'a b' 'c;d'` renders as `'a b' 'c;d'` — no shell injection.

Tokens after `--` forward unconditionally, bypassing any binding. `wt deploy -- --branch=foo` forwards `--branch=foo` to `{{ args }}` even though the template references `{{ branch }}`.

### Inspecting and previewing

- `wt config alias show <name>` prints the template.
- `wt config alias dry-run <name> [-- args...]` prints the rendered command.

{{ terminal(cmd="wt config alias show deploy|||wt config alias dry-run deploy|||wt config alias dry-run deploy -- --env=staging") }}

### Multi-step pipelines

`[[aliases.NAME]]` defines a pipeline. Each block runs serially; keys within a block run concurrently.

```toml
[[aliases.release]]
test = "cargo test"

[[aliases.release]]
build = "cargo build --release"
package = "cargo package --no-verify"

[[aliases.release]]
publish = "cargo publish {{ args }}"
```

`test` runs first, then `build` and `package` run together, then `publish` runs last. A step failure aborts the remaining steps. Every step sees the same `{{ args }}` and bound variables — `wt release -- --dry-run` forwards `--dry-run` to `publish` without affecting earlier steps.

### Sources and approval

When both user and project config define the same alias name, both run — user first, then project. Project-config aliases require approval on first run, same as project hooks. User-config aliases are trusted.

An alias that calls `wt switch` (or `wt switch --create`) changes the parent shell's directory, just like running `wt switch` directly.

### Recipe: rebase every worktree onto its upstream

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

`wt up` fetches all remotes, then iterates every worktree: skip if no upstream, skip if mid-rebase, otherwise rebase and auto-abort on conflict.

### Recipe: move or copy in-progress changes to a new worktree

`wt switch --create` lands you in a clean worktree. To carry staged, unstaged, and untracked changes along, wrap it with git's stash plumbing:

```toml
# .config/wt.toml
[aliases]
move-changes = '''
if git diff --quiet HEAD && test -z "$(git ls-files --others --exclude-standard)"; then
  wt switch --create {{ to }}
else
  git stash push --include-untracked --quiet
  wt switch --create {{ to }} --execute='git stash pop --index'
fi
'''
```

Run with `wt move-changes --to=feature-xyz`. The leading guard avoids touching a pre-existing stash when nothing is in flight; otherwise, `git stash push --include-untracked` captures everything, `wt switch --create` makes the new worktree, and `git stash pop --index` (via `--execute`) restores the changes there with the staged/unstaged split intact.

To copy instead of move (source keeps its changes too), add `git stash apply --index --quiet` right after the push. For staged-only flows, swap the stash for `git diff --cached` written to a tempfile and applied with `git apply --index` in the new worktree — that handles files where staged and unstaged hunks overlap on the same lines, where `git stash --staged` falls short.

### Recipe: tail a specific hook log

`wt config state logs --format=json` emits structured entries — `branch`, `source`, `hook_type`, `name`, `path`. Pipe through `jq` to resolve one entry, then wrap in an alias for quick access:

```toml
[aliases]
hook-log = '''
tail -f "$(wt config state logs --format=json | jq -r --arg name "{{ name | sanitize_hash }}" '
  .hook_output[]
  | select(.branch == "{{ branch | sanitize_hash }}" and .hook_type == "post-start" and .name == $name)
  | .path
' | head -1)"
'''
```

Run with `wt hook-log --name=<hook-name>` (e.g., `wt hook-log --name=server`) to tail the current worktree's `post-start` hook of that name. The `sanitize_hash` filter produces a filesystem-safe name with a hash suffix that keeps distinct originals unique — the same transformation Worktrunk applies on disk — so the alias resolves the right log even for branch and hook names containing characters like `/`.

## Custom subcommands

<span class="badge-experimental"></span>

Any executable named `wt-<name>` on `PATH` becomes available as `wt <name>` — the same pattern git uses for `git-foo`. Built-in commands and configured [aliases](#aliases) take precedence — `wt foo` resolves to the alias if `foo` is configured, otherwise to `wt-foo`.

{{ terminal(cmd="wt sync origin              # runs: wt-sync origin|||wt -C /tmp/repo sync        # -C is forwarded as the child's working directory") }}

Arguments pass through verbatim, stdio is inherited, and the child's exit code propagates unchanged. Custom subcommands don't have access to template variables.

### Examples

- [`worktrunk-sync`](https://github.com/pablospe/worktrunk-sync) — rebases stacked worktree branches in dependency order, inferring the tree from git history. Install with `cargo install worktrunk-sync`, then run as `wt sync`.
