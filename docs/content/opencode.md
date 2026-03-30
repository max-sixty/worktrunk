+++
title = "OpenCode Integration"
description = "Worktrunk plugin for OpenCode: activity tracking for wt list showing active OpenCode sessions."
weight = 24

[extra]
group = "Reference"
+++

The worktrunk OpenCode plugin provides activity tracking — status markers in `wt list` showing which worktrees have active OpenCode sessions (🤖 working, 💬 waiting).

## Installation

```bash
$ wt config opencode install
```

This writes the plugin to `~/.config/opencode/plugins/worktrunk.ts` (or `$OPENCODE_CONFIG_DIR/plugins/worktrunk.ts` if set).

To update an existing plugin:

```bash
$ wt config opencode install --yes
```

To remove:

```bash
$ wt config opencode uninstall
```

## Activity tracking

The plugin tracks OpenCode sessions with status markers in `wt list`:

- 🤖 — OpenCode is working (on `session.status`)
- 💬 — OpenCode is waiting for input (on `session.idle`)
- Cleared when the session ends (on `session.deleted`)

These markers appear in the Status column of `wt list`, making it easy to see which worktrees have active OpenCode sessions — useful when running multiple instances in parallel.

## How it works

The plugin is a TypeScript module loaded by OpenCode's plugin system.
It listens for session lifecycle events and calls `wt config state marker set/clear` to update the branch marker stored in git config.

The marker storage is agent-agnostic — the same `worktrunk.state.<branch>.marker` git config key is used by both the Claude Code and OpenCode plugins.
