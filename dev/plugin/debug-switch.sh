#!/bin/bash
# Debug script for wt switch zellij integration
# Run this INSIDE a zellij session to trace what's happening

set -e

echo "=== Environment ==="
echo "ZELLIJ=${ZELLIJ:-not set}"
echo "ZELLIJ_SESSION_NAME=${ZELLIJ_SESSION_NAME:-not set}"
echo "PWD=$(pwd)"

echo ""
echo "=== Worktree Info ==="
echo "worktree_root: $(git rev-parse --show-toplevel)"
echo "worktree_base: $(git rev-parse --git-common-dir | xargs dirname)"

echo ""
echo "=== Session Matching ==="
# Calculate what session name worktrunk expects
repo_root=$(git rev-parse --git-common-dir | xargs dirname | xargs realpath)
echo "repo_root (for session hash): $repo_root"

# Check if current session matches
if [[ "$ZELLIJ_SESSION_NAME" == wt:* ]]; then
    echo "Session IS a worktrunk session: $ZELLIJ_SESSION_NAME"
else
    echo "Session is NOT a worktrunk session: $ZELLIJ_SESSION_NAME"
    echo "wt switch will NOT use zellij tab management!"
    exit 1
fi

echo ""
echo "=== Plugin Communication Test ==="

# Current worktree path
current_wt=$(git rev-parse --show-toplevel)
echo "Current worktree: $current_wt"

# Test sync
echo ""
echo "Testing sync|$current_wt"
response=$(echo "sync|$current_wt" | timeout 5 zellij pipe --name wt 2>&1) || response="TIMEOUT/ERROR"
echo "Response: $response"

# Test select on same path (should return focused after sync)
echo ""
echo "Testing select|$(basename $current_wt)|$current_wt"
response=$(echo "select|$(basename $current_wt)|$current_wt" | timeout 5 zellij pipe --name wt 2>&1) || response="TIMEOUT/ERROR"
echo "Response: $response"

echo ""
echo "=== Plugin State (debug command) ==="
response=$(echo "debug" | timeout 5 zellij pipe --name wt 2>&1) || response="TIMEOUT/ERROR"
echo "State: $response"

echo ""
echo "=== Expected Behavior ==="
echo "If sync returned 'synced' and select returned 'focused', the plugin is working."
echo "If select returned 'not_found:...', the plugin lost state between calls."
echo ""
echo "The debug command shows tabs={N} where N is the TabUpdate count,"
echo "and tracked=[...] showing all paths currently in the path_to_tab map."
