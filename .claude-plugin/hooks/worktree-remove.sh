#!/usr/bin/env bash
# WorktreeRemove hook — removes worktrees via wt instead of raw git.
#
# CC contract: read JSON from stdin. Non-blocking (exit code ignored).
set -euo pipefail

input=$(cat)
worktree_path=$(echo "$input" | jq -r '.worktree_path // empty')

[[ -z "$worktree_path" ]] && exit 0

# Derive branch name from the worktree's HEAD.
branch=$(git -C "$worktree_path" rev-parse --abbrev-ref HEAD 2>/dev/null || true)

[[ -z "$branch" ]] && exit 0

# -D: force-delete branch (CC worktrees are ephemeral)
# --foreground: block until complete (CC fires this at session end)
wt remove "$branch" -D --foreground 2>/dev/null || true
