#!/usr/bin/env bash
# Claude Code WorktreeRemove hook: removes the worktree with `wt remove`.
#
# Resolves against the worktree path Claude Code hands over, never
# CLAUDE_PROJECT_DIR: the `claude agents` view spans repositories, so no single
# project dir is right for every session (#3754). Never force-deletes: Claude
# Code fires this on session exit for any clean worktree, and `-D` would
# discard committed-but-unpushed work (#2939).
p=$(jq -er .worktree_path) || exit 1
[ -e "$p/.git" ] || exit 0
bash "$CLAUDE_PLUGIN_ROOT/hooks/wt.sh" -C "$p" remove --foreground "$p"
