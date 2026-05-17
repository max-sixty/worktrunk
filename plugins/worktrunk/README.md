# Worktrunk Plugin for Claude Code

Git worktree management CLI integration with activity tracking.

## Features

1. **Configuration skill** — Guides LLM-powered commit message setup, project hooks (pre-start, pre-merge), and worktree path customization
2. **Activity tracking** — Shows which branches have active Claude sessions via indicators in `wt list`
3. **`/wt-switch-create` command** — Creates a worktrunk worktree and moves the current Claude session into it

## Examples

**Activity tracking across worktrees**

The plugin installs Claude Code hooks that track session activity per branch. When a prompt is submitted, the hook sets 🤖 on that branch. When Claude finishes and waits for input, it switches to 💬. When the session ends, the marker clears.

These markers appear in `wt list` output, making it easy to see which worktrees have active Claude sessions — useful when running multiple instances in parallel.

**Set up LLM commit message generation**

The configuration skill guides through configuring an AI tool (Claude Code, Codex, llm, or aichat) and adding `[commit.generation]` to the user config so `wt merge` can auto-generate commit messages.

**Add pre-start hooks to run npm install automatically**

The skill configures `.config/wt.toml` with project hooks. Pre-start hooks run when creating worktrees, pre-merge hooks validate before merging.

**Start work in a fresh worktree**

`/wt-switch-create fix-auth Investigate the 5-minute session timeout` creates a `fix-auth` worktree in worktrunk's normal sibling layout (`<repo>.fix-auth/`), switches the session into it, and starts the task there. On session exit the worktree is offered for removal — a worktree with uncommitted changes is kept rather than removed.
