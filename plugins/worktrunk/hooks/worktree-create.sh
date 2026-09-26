#!/usr/bin/env bash
# Claude Code WorktreeCreate hook: creates the worktree with `wt switch
# --create` and prints its path, which Claude Code reads as the hook's answer.
#
# Without pipefail the trailing `jq` exits 0 on a failed `wt`'s empty stdout,
# and Claude Code reports a successful hook with no path (#3545).
set -o pipefail
name=$(jq -er .name) || exit 1
cd "${CLAUDE_PROJECT_DIR:-.}" || exit 1
bash "$CLAUDE_PLUGIN_ROOT/hooks/wt.sh" switch --create "$name" --no-cd --format=json | jq -er .path
