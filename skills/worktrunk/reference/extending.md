# Extending Worktrunk

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
- **Project config** (`.config/wt.toml`) — shared with the team, requires [approval](https://worktrunk.dev/config/#wt-config-approvals) on first run

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

Aliases are custom commands invoked as `wt <name>`. They share the same template variables and approval model as hooks.

```toml
[aliases]
deploy = "make deploy BRANCH={{ branch }} ENV={{ env }}"
open = "open http://localhost:{{ branch | hash_port }}"
```

```bash
wt deploy --env=staging
wt deploy --dry-run --env=prod
```

`wt deploy` resolves `deploy` against configured aliases first, then falls through to a `wt-deploy` PATH binary if no alias matches. Built-in subcommands always take precedence — an alias named `list` or `switch` is unreachable.

### How arguments are routed

Tokens after the alias name fall into one of these buckets, decided by what the alias's template references:

| Token shape | Routes to |
|---|---|
| `--KEY=VALUE` or `--KEY VALUE` where the template references `{{ KEY }}` | Bound — `KEY` becomes the template value |
| `--KEY=VALUE` where the template doesn't reference `KEY` | Forwarded literally to `{{ args }}` |
| `--KEY` followed by another flag or end of args | Forwarded literally to `{{ args }}` |
| Bare positional (no `--` prefix) | Forwarded to `{{ args }}` |
| Anything after a literal `--` | Forwarded to `{{ args }}` regardless of shape |

Built-in flags (`--yes`/`-y`, `--dry-run`) are always recognized, so an alias can't shadow them. Built-in template variables (`branch`, `worktree_path`, `commit`, …) can be overridden — `--branch=override` for an alias referencing `{{ branch }}` binds to the user's value, but only inside the template; the worktree's actual branch is unchanged.

Hyphens in variable names are canonicalized to underscores at parse time. `--my-var=value` binds to `{{ my_var }}` because minijinja parses `{{ my-var }}` as subtraction.

### Escaping with `--`

Use `--` to forward a flag-shaped value literally instead of letting the parser bind it. Everything after `--` goes into `{{ args }}` verbatim:

```toml
[aliases]
search = "rg {{ args }}"
```

```bash
wt search -- --hidden --glob '*.rs' pattern  # --hidden and --glob reach rg, not the alias parser
```

### Forwarding positional arguments

Non-flag tokens after the alias name are forwarded to the template as `{{ args }}`. Bare `{{ args }}` renders as a space-joined, shell-escaped string ready to append to a command line — so `wt s some-branch` with `s = "wt switch {{ args }}"` expands to `wt switch some-branch`.

```toml
[aliases]
s = "wt switch {{ args }}"
```

```bash
wt s some-branch
wt s feature/api  # multiple tokens pass through in order
wt s 'has a space'  # spaces and metacharacters are escaped safely
```

Access elements with `{{ args[0] }}`, iterate with `{% for a in args %}…{% endfor %}`, or count with `{{ args | length }}`. Each element is individually shell-escaped, so `wt run 'a b' 'c;d'` splices in as `'a b' 'c;d'` without shell injection.

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

### Multi-step pipelines

Multi-step aliases run commands in order using `[[aliases.NAME]]` blocks. Each block is one step; multiple keys within a block run concurrently.

```toml
[[aliases.release]]
test = "cargo test"

[[aliases.release]]
build = "cargo build --release"
package = "cargo package --no-verify"

[[aliases.release]]
publish = "cargo publish"
```

`test` runs first, then `build` and `package` run together, then `publish` runs last. A step failure aborts the remaining steps.

### Sources and approval

When both user and project config define the same alias name, both run — user first, then project. Project-config aliases require approval on first run, same as project hooks. User-config aliases are trusted.

Inside an alias body, an inner `wt switch` (or `wt switch --create`) passes its `cd` through to the parent shell, so an alias wrapping `wt switch --create` lands the shell in the new worktree just like running it directly.

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

[experimental]

Any executable named `wt-<name>` on `PATH` becomes available as `wt <name>` — the same pattern git uses for `git-foo`. Built-in commands and configured [aliases](#aliases) take precedence — `wt foo` resolves to the alias if `foo` is configured, otherwise to `wt-foo`.

```bash
wt sync origin              # runs: wt-sync origin
wt -C /tmp/repo sync        # -C is forwarded as the child's working directory
```

Arguments pass through verbatim, stdio is inherited, and the child's exit code propagates unchanged. Custom subcommands don't have access to template variables.

### Examples

- [`worktrunk-sync`](https://github.com/pablospe/worktrunk-sync) — rebases stacked worktree branches in dependency order, inferring the tree from git history. Install with `cargo install worktrunk-sync`, then run as `wt sync`.
