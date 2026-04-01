# Worktrunk Plugin for OpenCode

Activity tracking integration for `wt list`.

## Installation

```bash
wt config plugins opencode install
```

This copies the plugin to `~/.config/opencode/plugins/worktrunk.ts` (or `$OPENCODE_CONFIG_DIR/plugins/worktrunk.ts`).

## What it does

The plugin tracks OpenCode session activity per branch using status markers:

- 🤖 — Agent is working (on `session.status`)
- 💬 — Agent is waiting for input (on `session.idle`)
- Cleared when session ends (on `session.deleted`)

These markers appear in `wt list` output, showing which worktrees have active OpenCode sessions.

## Manual installation

Copy `.opencode-plugin/worktrunk.ts` to `~/.config/opencode/plugins/worktrunk.ts`.
