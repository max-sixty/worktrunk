# Worktrunk Plugin for Claude Code

Git worktree management CLI integration with activity tracking.

## Features

1. **Configuration skill** — Guides LLM-powered commit message setup, project hooks (post-create, pre-merge), and worktree path customization
2. **Activity tracking** — Shows which branches have active Claude sessions via indicators in `wt list`

## Examples

**Activity tracking across worktrees**

The plugin installs Claude Code hooks that track session activity per branch. When a prompt is submitted, the hook sets 🤖 on that branch. When Claude finishes and waits for input, it switches to 💬. When the session ends, the marker clears.

These markers appear in `wt list` output, making it easy to see which worktrees have active Claude sessions — useful when running multiple instances in parallel.

**Set up LLM for commit message generation**

The configuration skill guides through installing the `llm` CLI tool, setting API keys, and adding `[commit-generation]` to the user config so `wt merge` can auto-generate commit messages.

**Add post-create hooks to run npm install automatically**

The skill configures `.config/wt.toml` with project hooks. Post-create hooks run when creating worktrees, pre-merge hooks validate before merging.
