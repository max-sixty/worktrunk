#!/bin/bash
# Test script for wt-bridge protocol
# Run from inside a wt:* zellij session

set -e

SESSION="${ZELLIJ_SESSION_NAME:?Must run inside a zellij session}"
PLUGIN="file:~/.config/zellij/plugins/wt-bridge.wasm"

if [[ ! "$SESSION" =~ ^wt: ]]; then
    echo "Warning: Session '$SESSION' is not a wt:* session"
fi

echo "=== wt-bridge protocol test (session: $SESSION) ==="

# Sync Tab1 with pathA
echo "1. Syncing /tmp/pathA..."
response=$(echo "sync|/tmp/pathA" | zellij -s "$SESSION" pipe --plugin "$PLUGIN" --name wt)
echo "   Response: $response"
[[ "$response" == "synced" ]] && echo "   ✅" || echo "   ❌ Expected: synced"

# Go to Tab2 and sync with pathB
echo "2. Switching to tab 2 and syncing /tmp/pathB..."
zellij -s "$SESSION" action go-to-tab 2
# Wait for TabUpdate event to propagate to plugin
sleep 0.5
response=$(echo "sync|/tmp/pathB" | zellij -s "$SESSION" pipe --plugin "$PLUGIN" --name wt)
echo "   Response: $response"
[[ "$response" == "synced" ]] && echo "   ✅" || echo "   ❌ Expected: synced"

# Select pathA (should return focused:1)
echo "3. Selecting /tmp/pathA..."
response=$(echo "select|test|/tmp/pathA" | zellij -s "$SESSION" pipe --plugin "$PLUGIN" --name wt)
echo "   Response: $response"
[[ "$response" == "focused:1" ]] && echo "   ✅" || echo "   ❌ Expected: focused:1"

# Actually switch to verify it works
if [[ "$response" =~ ^focused:([0-9]+)$ ]]; then
    tab="${BASH_REMATCH[1]}"
    echo "   Switching to tab $tab..."
    zellij -s "$SESSION" action go-to-tab "$tab"
    echo "   ✅ Tab switch executed"
fi

# Select pathB (should return focused:2)
echo "4. Selecting /tmp/pathB..."
response=$(echo "select|test|/tmp/pathB" | zellij -s "$SESSION" pipe --plugin "$PLUGIN" --name wt)
echo "   Response: $response"
[[ "$response" == "focused:2" ]] && echo "   ✅" || echo "   ❌ Expected: focused:2"

if [[ "$response" =~ ^focused:([0-9]+)$ ]]; then
    tab="${BASH_REMATCH[1]}"
    echo "   Switching to tab $tab..."
    zellij -s "$SESSION" action go-to-tab "$tab"
    echo "   ✅ Tab switch executed"
fi

echo ""
echo "=== Test complete ==="
