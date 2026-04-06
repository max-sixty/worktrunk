#!/usr/bin/env bash
# WorktreeCreate hook — creates worktrees via wt instead of raw git.
#
# CC contract: read JSON from stdin, print absolute worktree path to stdout, exit 0.
# Only stdout is read by CC; all wt output goes to stderr (redirected to /dev/null).
set -euo pipefail

input=$(cat)

# CC input field names: see https://docs.anthropic.com/en/docs/claude-code/hooks
branch=$(echo "$input" | jq -r '.worktree_name // .branch // empty')

if [[ -z "$branch" ]]; then
  echo "Error: no branch in WorktreeCreate input" >&2
  exit 1
fi

# Create worktree via wt. --format=json gives us the path reliably.
# --no-cd: don't attempt shell directory change
# Fail if the branch already exists — CC isolation expects a fresh worktree.
result=$(wt switch --create "$branch" --no-cd --format=json 2>/dev/null) || exit 1

echo "$result" | jq -r '.path'
