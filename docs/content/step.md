+++
title = "wt step"
weight = 16

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt step --help-page` — edit cli.rs to update -->

Run individual git workflow operations: commits, squashes, rebases, and pushes.

## Examples

Commit with LLM-generated message:

```bash
wt step commit
```

Manual merge workflow with review between steps:

```bash
wt step commit
wt step squash
# Review the squashed commit
wt step rebase
wt step push
```

## Operations

- `commit` — Stage and commit with [LLM-generated message](@/llm-commits.md)
- `squash` — Squash all branch commits into one with [LLM-generated message](@/llm-commits.md)
- `rebase` — Rebase onto target branch
- `push` — Push to target branch (default: main)
- `for-each` — [experimental] Run a command in every worktree

## See also

- [wt merge](@/merge.md) — Runs commit → squash → rebase → hooks → push → cleanup automatically
- [wt hook](@/hook.md) — Run hooks independently

## Command reference

```
wt step - Run individual workflow operations

Usage: wt step [OPTIONS] <COMMAND>

Commands:
  commit    Commit changes with LLM commit message
  squash    Squash commits down to target
  push      Push changes to local target branch
  rebase    Rebase onto target
  for-each  [experimental] Run command in each worktree

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info
```

## wt step for-each

Executes a command sequentially in every worktree with real-time output. Continues on failure and shows a summary at the end.

Context JSON is piped to stdin for scripts that need structured data.

### Template variables

All variables are shell-escaped:

| Variable | Description |
|----------|-------------|
| `{{ branch }}` | Branch name (slashes replaced with dashes) |
| `{{ worktree }}` | Absolute path to the worktree |
| `{{ worktree_name }}` | Worktree directory name |
| `{{ repo }}` | Repository name |
| `{{ repo_root }}` | Absolute path to the main repository root |
| `{{ commit }}` | Current HEAD commit SHA (full) |
| `{{ short_commit }}` | Current HEAD commit SHA (7 chars) |
| `{{ default_branch }}` | Default branch name (e.g., "main") |
| `{{ remote }}` | Primary remote name (e.g., "origin") |
| `{{ remote_url }}` | Primary remote URL |
| `{{ upstream }}` | Upstream tracking branch, if configured |

### Examples

Check status across all worktrees:

```bash
wt step for-each -- git status --short
```

Run npm install in all worktrees:

```bash
wt step for-each -- npm install
```

Use branch name in command:

```bash
wt step for-each -- "echo Branch: {{ branch }}"
```

Pull updates in worktrees with upstreams (skips others):

```bash
git fetch --prune && wt step for-each -- '[ "$(git rev-parse @{u} 2>/dev/null)" ] || exit 0; git pull --autostash'
```

Note: This command is experimental and may change in future versions.

### Command reference

```
wt step for-each - [experimental] Run command in each worktree

Usage: wt step for-each [OPTIONS] -- <ARGS>...

Arguments:
  <ARGS>...
          Command template (see --help for all variables)

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info
```

<!-- END AUTO-GENERATED from `wt step --help-page` -->
