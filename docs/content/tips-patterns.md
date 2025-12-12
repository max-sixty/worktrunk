+++
title = "Tips & Patterns"
weight = 24

[extra]
group = "Reference"
+++

Practical recipes for common Worktrunk workflows.

## Alias for new worktree + agent

Create a worktree and launch Claude in one command:

```bash
alias wsc='wt switch --create --execute=claude'
wsc new-feature  # Creates worktree, runs hooks, launches Claude
```

## Eliminate cold starts

`post-create` hooks install deps and copy caches. On macOS, use copy-on-write for instant cache cloning:

```toml
[post-create]
"cache" = "cp -c -r ../.cache .cache"  # APFS clones (instant, no disk space)
"install" = "npm ci"
```

See [Worktrunk's own `.config/wt.toml`](https://github.com/max-sixty/worktrunk/blob/main/.config/wt.toml) for a complete example.

## Local CI gate

`pre-merge` hooks run before merging. Failures abort the merge:

```toml
[pre-merge]
"lint" = "uv run ruff check"
"test" = "uv run pytest"
```

This catches issues locally before pushing — like having CI run on your machine.

## Track agent status

Custom emoji markers show agent state in `wt list`. The Claude Code plugin sets these automatically:

```text
+ feature-api      ↑  🤖              ↑1      ./repo.feature-api
+ review-ui      ? ↑  💬              ↑1      ./repo.review-ui
```

- `🤖` — Claude is working
- `💬` — Claude is waiting for input

Set status manually for any workflow:

```bash
wt config var set marker "🚧"                   # Current branch
wt config var set marker "✅" --branch feature  # Specific branch
git config worktrunk.marker.feature "💬"        # Direct git config
```

See [Claude Code Integration](@/claude-code.md#installation) for plugin installation.

## Monitor CI across branches

```bash
wt list --full --branches
```

Shows PR/CI status for all branches, including those without worktrees. CI indicators are clickable links to the PR page.

## JSON API

```bash
wt list --format=json
```

Structured output for dashboards, statuslines, and scripts. See [wt list](@/list.md) for query examples.

## Task runners in hooks

Reference Taskfile/Justfile/Makefile in hooks:

```toml
[post-create]
"setup" = "task install"

[pre-merge]
"validate" = "just test lint"
```

## Shortcuts

Special arguments work across all commands—see [wt switch](@/switch.md#shortcuts) for the full list.

```bash
wt switch --create hotfix --base=@       # Branch from current HEAD
wt switch -                              # Switch to previous worktree
wt remove @                              # Remove current worktree
```

## Stacked branches

Branch from current HEAD instead of main:

```bash
wt switch --create feature-part2 --base=@
```

Creates a worktree that builds on the current branch's changes.
