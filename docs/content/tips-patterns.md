+++
title = "Tips & Patterns"
weight = 5
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

See [`.config/wt.toml`](https://github.com/max-sixty/worktrunk/blob/main/.config/wt.toml) on GitHub for a complete example.

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

```
+ feature-api      ↑  🤖              ↑1      ./repo.feature-api
+ review-ui      ? ↑  💬              ↑1      ./repo.review-ui
```

- `🤖` — Claude is working
- `💬` — Claude is waiting for input

Set status manually for any workflow:

```bash
wt config status set "🚧"                    # Current branch
wt config status set "✅" --branch feature   # Specific branch
git config worktrunk.status.feature "💬"     # Direct git config
```

See [Claude Code Integration](/advanced/#claude-code-integration) for plugin installation.

## Monitor CI across branches

```bash
wt list --full --branches
```

Shows PR/CI status for all branches, including those without worktrees. The CI column links to PR pages in terminals with hyperlink support.

## JSON API

```bash
wt list --format=json
```

Structured output for dashboards, statuslines, and scripts. See [wt list](/commands/#wt-list) for query examples.

## Task runners in hooks

Reference Taskfile/Justfile/Makefile in hooks:

```toml
[post-create]
"setup" = "task install"

[pre-merge]
"validate" = "just test lint"
```

## Shortcuts

Special arguments work across all commands:

| Shortcut | Meaning |
|----------|---------|
| `^` | default branch (main/master) |
| `@` | current branch/worktree |
| `-` | previous worktree (like `cd -`) |

Examples:

```bash
wt switch --create hotfix --base=@       # Branch from current HEAD
wt switch --create feature --base=^      # Branch from main (default)
wt switch -                              # Switch to previous worktree
wt remove @                              # Remove current worktree
```

## Stacked branches

Branch from current HEAD instead of main:

```bash
wt switch --create feature-part2 --base=@
```

Creates a worktree that builds on the current branch's changes.
