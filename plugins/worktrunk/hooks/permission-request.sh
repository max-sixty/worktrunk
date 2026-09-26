#!/usr/bin/env bash
# Claude Code PermissionRequest hook: approves an EnterWorktree into a
# worktrunk-managed worktree, and otherwise marks the session 💬 while the
# dialog waits.
#
# One hook does both because Claude Code runs every matching hook in
# parallel; see skills/wt-switch-create/rationale.md.
bash "$CLAUDE_PLUGIN_ROOT/hooks/wt.sh" config plugins claude approve-enter-worktree ||
    bash "$CLAUDE_PLUGIN_ROOT/hooks/marker.sh" set 💬
