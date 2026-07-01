+++
title = "Extending Worktrunk"
description = "Three ways to add custom behavior: hooks for lifecycle automation, aliases for reusable commands, and custom subcommands for standalone tools."
weight = 21

[extra]
group = "Reference"
+++

Worktrunk has three extension mechanisms.

**[Hooks](#hooks)** are shell commands that run automatically at lifecycle events (switching, starting, committing, merging, removing). Defined in TOML.

**[Aliases](#aliases)** are reusable shell commands invoked as `wt <name>`. Defined in TOML.

**[Custom subcommands](#custom-subcommands)** are standalone executables invoked as `wt <name>`. Drop `wt-foo` on `PATH` and it becomes `wt foo`.

| | Hooks | Aliases | Custom subcommands |
|---|---|---|---|
| **Trigger** | Automatic (lifecycle events) | Manual (`wt <name>`) | Manual (`wt <name>`) |
| **Defined in** | TOML config | TOML config | Any executable on `PATH` |
| **Template variables** | Yes | Yes | No |
| **Shareable via repo** | `.config/wt.toml` | `.config/wt.toml` | Distribute the binary |
| **Language** | Shell commands | Shell commands | Any |

Hooks and aliases live in the same TOML config and share the [template engine](@/hook.md#template-variables). User config is trusted; project config requires approval on first run. When both define the same name, both run (user first).

## Hooks

Ten hooks cover five lifecycle events:

| Event | `pre-` (blocking) | `post-` (background) |
|-------|-------------------|---------------------|
| **switch** | `pre-switch` | `post-switch` |
| **start** | `pre-start` | `post-start` |
| **commit** | `pre-commit` | `post-commit` |
| **merge** | `pre-merge` | `post-merge` |
| **remove** | `pre-remove` | `post-remove` |

`pre-*` hooks block: failure aborts the operation. `post-*` hooks run in the background.

```toml
[pre-start]
deps = "npm ci"

[post-start]
server = "npm run dev -- --port {{ branch | hash_port }}"

[pre-merge]
test = "npm test"
```

See [`wt hook`](@/hook.md) for the full reference and built-in recipes (dev server per worktree, database per worktree, progressive validation). [Tips & Patterns](@/tips-patterns.md) has more.

## Aliases

Aliases are configured under `[aliases]`:

```toml
[aliases]
deploy = "fly deploy --config=fly.{{ env }}.toml --app=myapp-{{ branch }}"
open = "open http://localhost:{{ branch | hash_port }}"
since-main = "git log --oneline {{ default_branch }}..HEAD"
```

{{ terminal(cmd="wt deploy --env=staging|||wt open") }}

`wt <name>` resolves to a built-in first, then an alias, then a [custom subcommand](#custom-subcommands).

### Templates

Aliases use the same [template engine as hooks](@/hook.md#template-variables): variables, [filters](@/hook.md#worktrunk-filters), [functions](@/hook.md#worktrunk-functions), and [`--KEY=VALUE` smart routing](@/hook.md#passing-values) (bind if the template references `KEY`, else forward to `{{ args }}`). For example, `wt deploy --env=staging` sets `{{ env }}`.

Alias templates add `{{ args }}` for positional CLI arguments. Operation-context variables (`target`, `base`, `pr_number`) aren't auto-populated, but can still be bound with `--KEY=VALUE`.

### Positional arguments

`{{ args }}` renders as a space-joined, shell-escaped string, ready to splice into a command:

```toml
[aliases]
s = "wt switch {{ args }}"
```

{{ terminal(cmd="wt s some-branch|||wt s feature/api|||wt s 'has a space'") }}

For indexing (`{{ args[0] }}`), looping, and counting, see [Passing values](@/hook.md#passing-values).

Tokens after `--` forward unconditionally, bypassing any binding. Writing `wt deploy -- --branch=foo` forwards the literal `--branch=foo` to `{{ args }}` even though the template references `{{ branch }}`.

An alias that forwards `{{ args }}` to a `wt` command — like `co = "wt switch {{ args }}"` or `cm = "wt step commit {{ args }}"` — inherits that command's argument and flag completion, so `wt co <Tab>` completes branches the same way `wt switch <Tab>` does.

### Inspecting and previewing

- `wt config alias show <name>` prints the template.
- `wt config alias dry-run <name> [-- args...]` prints the rendered command.

{{ terminal(cmd="wt config alias show deploy|||wt config alias dry-run deploy|||wt config alias dry-run deploy -- --env=staging") }}

### Multi-step pipelines

`[[aliases.NAME]]` defines a pipeline using the [same `[[block]]` semantics as hooks](@/hook.md#hook-forms): blocks run in order, keys within a block run concurrently, and a step failure aborts the remainder.

```toml
[[aliases.release]]
test = "cargo test"

[[aliases.release]]
build = "cargo build --release"
package = "cargo package --no-verify"

[[aliases.release]]
publish = "cargo publish {{ args }}"
```

Every step sees the same `{{ args }}` and bound variables. `wt release -- --dry-run` forwards `--dry-run` to `publish` without affecting earlier steps.

### Changing directory

`wt switch`, `wt merge` (when it leaves the removed source), and `wt remove` of the current worktree change the parent shell's directory even when invoked from an alias; the Worktrunk shell integration propagates the change through. Other shell state doesn't persist: the alias runs in a subshell, so `cd`, `export`, and similar commands only affect that subshell.

### Deferring expansion to a nested `wt` command

A `wt step for-each` alias that prints the same branch in every worktree is rendering `{{ branch }}` too early. An alias body renders once at dispatch, in the invoking worktree, so a bare `{{ branch }}` is baked to that worktree's branch before for-each iterates. (`wt config alias dry-run <name>` shows the rendered body, with the value already baked in.)

`{% raw %}…{% endraw %}` defers the variable: it survives the dispatch render as a literal `{{ branch }}`, and for-each expands it per worktree. One catch for `for-each`: the deferred `{{ branch }}` has spaces, so the alias body's `sh -c` splits it into `{{`, `branch`, `}}` before for-each sees it (`Failed to expand for-each argument: syntax error`). Give for-each its own `sh -c '…'` to keep the value one token:

```toml
[aliases]
show-branches = "wt step for-each -- sh -c 'echo {% raw %}{{ branch }}{% endraw %}'"
```

`wt show-branches` prints each worktree's own branch.

`wt switch --execute` defers the same way, without the extra wrapper: its `--execute '…'` argument is already a single quoted string, so only `{% raw %}` is needed. Here `{{ worktree_path }}` expands against the worktree being created, not the one the alias ran from:

```toml
[aliases]
echo-target = "wt switch {{ args }} --no-cd --execute 'echo {% raw %}{{ worktree_path }}{% endraw %}'"
```

A repo-level variable like `{{ default_branch }}` needs no deferral: it is identical in every worktree, so a bare `{{ default_branch }}` is already correct everywhere.

### Recipe: rebase every worktree onto its upstream

```toml
[aliases]
up = '''
git fetch --all --prune && wt step for-each -- sh -c '
  git rev-parse --verify -q @{u} >/dev/null || exit 0
  g=$(git rev-parse --git-dir)
  test -d "$g/rebase-merge" -o -d "$g/rebase-apply" && exit 0
  git update-index --refresh -q >/dev/null || true
  git rebase @{u} --no-autostash || git rebase --abort
''''
```

`wt up` fetches all remotes, then iterates every worktree: skip if no upstream, skip if mid-rebase, refresh the index to drop stale stat entries, then rebase and auto-abort on conflict. It rebases onto git-native `@{u}` rather than a `{{ … }}` template, so git resolves each worktree's own upstream and there is nothing to defer.

### Recipe: move or copy in-progress changes to a new worktree

`wt switch --create` lands you in a clean worktree. To carry staged, unstaged, and untracked changes along, pair it with `git stash`:

```toml
# .config/wt.toml
[aliases]
move-changes = '''
if git diff --quiet HEAD && test -z "$(git ls-files --others --exclude-standard)"; then
  wt switch --create {{ to }} --execute="{{ args }}"
else
  git stash push --include-untracked --quiet
  wt switch --create {{ to }} --execute="git stash pop --index; {{ args }}"
fi
'''
```

Run with `wt move-changes --to=feature-xyz`. The guard skips the stash when nothing is in flight; otherwise `git stash push` captures everything and `--execute` pops it in the new worktree with the staged/unstaged split intact. Anything after `--` runs in the new worktree after pop. For example, `wt move-changes --to=feature-xyz -- claude` opens Claude there.

To copy instead of move, add `git stash apply --index --quiet` right after the push.

### Recipe: tail a specific hook log

`wt config state logs --format=json` emits structured entries (`branch`, `source`, `hook_type`, `name`, `path`). Pipe through `jq` to resolve one entry, then wrap in an alias for quick access:

```toml
[aliases]
hook-log = '''
tail -f "$(wt config state logs --format=json | jq -r --arg name "{{ name | sanitize_hash }}" --arg kind "{{ kind }}" '
  .hook_output[]
  | select(.branch == "{{ branch | sanitize_hash }}" and .hook_type == $kind and .name == $name)
  | .path
' | head -1)"
'''
```

Run with `wt hook-log --kind=post-start --name=server` to tail the log for the `server` hook on the current branch. `--kind` picks the hook type; the branch is pulled from the current worktree via `{{ branch }}`. `sanitize_hash` rewrites `branch` and `name` to filesystem-safe forms with a hash suffix that keeps distinct originals unique (the same transformation Worktrunk applies on disk), so the alias resolves the right log even when either contains characters like `/`.

## Custom subcommands

<span class="badge-experimental"></span>

Any executable named `wt-<name>` on `PATH` becomes available as `wt <name>`, the same pattern git uses for `git-foo`. Built-in commands and [aliases](#aliases) take precedence.

{{ terminal(cmd="wt sync origin              # runs: wt-sync origin|||wt -C /tmp/repo sync        # -C is forwarded as the child's working directory") }}

Arguments pass through verbatim, stdio is inherited, and the child's exit code propagates unchanged.

### Examples

- [`worktrunk-sync`](https://github.com/pablospe/worktrunk-sync): rebases stacked worktree branches in the dependency order inferred from git history. Install with `cargo install worktrunk-sync`, then run as `wt sync`.

## Reference: hooks vs. aliases

Aside from the differences below, hooks and aliases behave the same.

<details>
<summary>Interface differences</summary>

| Axis | Hooks | Aliases |
|------|-------|---------|
| Invocation | `wt hook <type> [args...]` (nested under the `hook` built-in) | `wt <name> [args...]` (top-level) |
| Bare positionals | Filter names (`wt hook pre-merge test build` runs only `test` and `build`) | Forwarded to `{{ args }}` |
| Reach `{{ args }}` from positionals | Must use `--` (`wt hook pre-merge -- extra`) | Any bare positional lands there |
| Approval skip flag | Post-subcommand `--yes` / `-y` supported (`wt hook pre-merge --yes`) | Only the global form (`wt -y <alias>`); post-alias `--yes` falls through to `{{ args }}` |
| Source discrimination | `user:` / `project:` / `user:name` / `project:name` filter syntax | Run user first, then project; no filter syntax |
| Force-bind escape | `--var KEY=VALUE` (deprecated in favor of `--KEY=VALUE`, but still force-binds) | None; smart routing is the only path |
| `--help` | `wt hook --help` lists hook types; `wt hook <type> --help` shows flags and arguments for that type | The template body is the documentation: `wt <alias> --help` redirects to `wt config alias show` / `dry-run`. `wt --help` and `wt step --help` list configured aliases alongside built-in commands |
| Inspection | `wt hook show [type] [--expanded]` | `wt config alias show <name>` / `wt config alias dry-run <name>` |
| Stdin | Foreground (`pre-*`) inherit parent stdin, same as aliases (interactive prompts work); background (`post-*`) receive all template variables as JSON (parse with `json.load(sys.stdin)`) | Inherits parent stdin (pipes pass through; interactive TUIs like `wt switch` keep the tty) |
| Template-context extras | `hook_type`, `hook_name`, per-type operation vars (`base`, `target`, `pr_number`, …) | `args` on top of the shared base variables |

</details>
