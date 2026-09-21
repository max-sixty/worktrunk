# Worktrunk Agent Plugin

Git worktree management CLI integration with activity tracking.

Requires the `wt` CLI ([worktrunk.dev](https://worktrunk.dev)). Claude Code's
worktree-lifecycle hooks also require `jq`.

## Features

1. **Configuration skill** — Guides LLM-powered commit message setup, project hooks (pre-start, pre-merge), and worktree path customization
2. **Activity tracking** — Shows which branches have active local agent sessions via indicators in `wt list`
3. **Claude Code worktree integration** — Routes Claude's worktree lifecycle through Worktrunk and provides `/wt-switch-create`

## Examples

**Activity tracking across worktrees**

The plugin installs native Claude Code, Cursor, and Codex hooks that track
session activity per branch. When a prompt is submitted, the hook sets 🤖 on
that branch. When the agent finishes and waits for input, it switches to 💬.
When the local session ends, the marker clears.

These markers appear in `wt list` output, making it easy to see which
worktrees have active agent sessions — useful when running multiple instances
in parallel.

**Set up LLM commit message generation**

The configuration skill guides through configuring an AI tool (Claude Code, Codex, llm, or aichat) and adding `[commit.generation]` to the user config so `wt merge` can auto-generate commit messages.

**Add pre-start hooks to run npm install automatically**

The skill configures `.config/wt.toml` with project hooks. Pre-start hooks run when creating worktrees, pre-merge hooks validate before merging.

**Start work in a fresh worktree**

`/wt-switch-create fix-auth -- Investigate the 5-minute session timeout` creates a `fix-auth` worktree in worktrunk's normal sibling layout (`<repo>.fix-auth/`), switches the session into it, and starts the task there. The branch name is optional (`/wt-switch-create -- <task>`). The worktree persists after the session — merge or remove it with `wt merge` / `wt remove` like any other.
