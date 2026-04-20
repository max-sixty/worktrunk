# Extending Worktrunk

Worktrunk has three extension mechanisms.

**[Hooks](#hooks)** run shell commands at lifecycle events — creating a worktree, merging, removing. They're configured in TOML and run automatically.

**[Aliases](#aliases)** define reusable commands invoked as `wt <name>`.

**[Custom subcommands](#custom-subcommands)** are standalone executables. Drop `wt-foo` on `PATH` and it becomes `wt foo`. No configuration needed.

| | Hooks | Aliases | Custom subcommands |
|---|---|---|---|
| **Trigger** | Automatic (lifecycle events) | Manual (`wt <name>`) | Manual (`wt <name>`) |
| **Defined in** | TOML config | TOML config | Any executable on `PATH` |
| **Template variables** | Yes | Yes | No |
| **Shareable via repo** | `.config/wt.toml` | `.config/wt.toml` | Distribute the binary |
| **Language** | Shell commands | Shell commands | Any |

Hooks and aliases share the TOML config file, the [template engine](https://worktrunk.dev/hook/#template-variables), the `[[block]]` pipeline syntax (blocks run in order, keys within a block run concurrently), and the approval model: user config is trusted; project config requires approval on first run. When both sources define the same name, both run — user first.

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

```toml
[pre-start]
deps = "npm ci"

[post-start]
server = "npm run dev -- --port {{ branch | hash_port }}"

[pre-merge]
test = "npm test"
```

See [`wt hook`](https://worktrunk.dev/hook/) for the full configuration reference — TOML forms, template variables and filters, and built-in recipes (dev server per worktree, database per worktree, progressive validation). [Tips & Patterns](https://worktrunk.dev/tips-patterns/) has more.

## Aliases

`[aliases]` defines commands invoked as `wt <name>`.

```toml
[aliases]
deploy = "fly deploy --config=fly.{{ env }}.toml --app=myapp-{{ branch }}"
open = "open http://localhost:{{ branch | hash_port }}"
since-main = "git log --oneline {{ default_branch }}..HEAD"
```

```bash
wt deploy --env=staging
wt open
```

`wt <name>` resolves to a built-in first, then an alias, then a [custom subcommand](#custom-subcommands).

### Templates

Alias templates have access to the full [variable and filter reference](https://worktrunk.dev/hook/#template-variables), plus `{{ args }}` for positional CLI arguments. Operation-context variables (`target`, `base`, `pr_number`) aren't populated in aliases since there's no operation in progress.

`--KEY=VALUE` (or `--KEY VALUE`) binds `KEY` whenever `{{ KEY }}` appears in the template — `wt deploy --env=staging` sets `{{ env }}` to `staging`. Everything else joins `{{ args }}` (see [Positional arguments](#positional-arguments)).

Built-in variables can be overridden: `--branch=foo` sets `{{ branch }}` inside the template — the worktree's actual branch doesn't move.

Hyphens in keys become underscores: `--my-var=x` sets `{{ my_var }}`.

### Positional arguments

`{{ args }}` renders as a space-joined, shell-escaped string — ready to splice into a command:

```toml
[aliases]
s = "wt switch {{ args }}"
```

```bash
wt s some-branch
wt s feature/api
wt s 'has a space'
```

Index with `{{ args[0] }}`, loop with `{% for a in args %}…{% endfor %}`, count with `{{ args | length }}`. Each element is escaped individually, so `wt run 'a b' 'c;d'` renders as `'a b' 'c;d'` — no shell injection.

Tokens after `--` forward unconditionally, bypassing any binding. `wt deploy -- --branch=foo` forwards `--branch=foo` to `{{ args }}` even though the template references `{{ branch }}`.

### Inspecting and previewing

- `wt config alias show <name>` prints the template.
- `wt config alias dry-run <name> [-- args...]` prints the rendered command.

```bash
wt config alias show deploy
wt config alias dry-run deploy
wt config alias dry-run deploy -- --env=staging
```

### Multi-step pipelines

`[[aliases.NAME]]` defines a pipeline using the same `[[block]]` semantics as hooks:

```toml
[[aliases.release]]
test = "cargo test"

[[aliases.release]]
build = "cargo build --release"
package = "cargo package --no-verify"

[[aliases.release]]
publish = "cargo publish {{ args }}"
```

A step failure aborts the remaining steps. Every step sees the same `{{ args }}` and bound variables — `wt release -- --dry-run` forwards `--dry-run` to `publish` without affecting earlier steps.

### Changing directory

`wt` commands that change the parent shell's directory — `wt switch`, `wt merge` (leaving the removed source), `wt remove` of the current worktree — still do so when invoked from an alias; the Worktrunk shell integration propagates the change through. Other shell state doesn't persist: the alias runs in a subshell, so `cd`, `export`, and similar commands only affect that subshell.

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

[experimental]

Any executable named `wt-<name>` on `PATH` becomes available as `wt <name>` — the same pattern git uses for `git-foo`. Built-in commands and configured [aliases](#aliases) take precedence — `wt foo` resolves to the alias if `foo` is configured, otherwise to `wt-foo`.

```bash
wt sync origin              # runs: wt-sync origin
wt -C /tmp/repo sync        # -C is forwarded as the child's working directory
```

Arguments pass through verbatim, stdio is inherited, and the child's exit code propagates unchanged. Custom subcommands don't have access to template variables.

### Examples

- [`worktrunk-sync`](https://github.com/pablospe/worktrunk-sync) — rebases stacked worktree branches in dependency order, inferring the tree from git history. Install with `cargo install worktrunk-sync`, then run as `wt sync`.

## Reference: hooks vs. aliases

Hooks and aliases share a template-variable model and a smart-routing rule for `--KEY=VALUE` shorthand (bind if the template references the key, else forward to `{{ args }}`), so a pattern learned on one surface mostly transfers to the other. A few things differ.

<details>
<summary>Interface differences</summary>

| Axis | Hooks | Aliases |
|------|-------|---------|
| Invocation | `wt hook <type> [args...]` — nested under the `hook` built-in | `wt <name> [args...]` — top-level |
| Bare positionals | Filter names (`wt hook pre-merge test build` runs only `test` and `build`) | Forwarded to `{{ args }}` |
| Reach `{{ args }}` from positionals | Must use `--` (`wt hook pre-merge -- extra`) | Any bare positional lands there |
| Approval skip flag | Post-subcommand `--yes` / `-y` supported (`wt hook pre-merge --yes`) | Only the global form (`wt -y <alias>`); post-alias `--yes` falls through to `{{ args }}` |
| Source discrimination | `user:` / `project:` / `user:name` / `project:name` filter syntax | Run user first, then project; no filter syntax |
| Force-bind escape | `--var KEY=VALUE` (deprecated — prefer `--KEY=VALUE` — but still force-binds) | None — smart routing is the only path |
| `--help` | `wt hook --help` lists hook types; `wt hook <type> --help` shows flags and arguments for that type | The template body is the documentation — `wt <alias> --help` redirects to `wt config alias show` / `dry-run`; `wt --help` and `wt step --help` list configured aliases alongside built-in commands |
| Inspection | `wt hook show [type] [--expanded]` | `wt config alias show <name>` / `wt config alias dry-run <name>` |
| Template-context extras | `hook_type`, `hook_name`, per-type operation vars (`base`, `target`, `pr_number`, …) | `args` on top of the shared base variables |

</details>
