#!/bin/bash
# Build, install, and verify the wt-bridge zellij plugin.
#
# Usage:
#   ./dev/plugin/setup.sh          # Build and install
#   ./dev/plugin/setup.sh --clean  # Also kill sessions and clear cache

set -e
cd "$(dirname "$0")/../.."

PLUGIN=~/.config/zellij/plugins/wt-bridge.wasm
LAYOUT=~/.config/zellij/layouts/worktrunk.kdl
CONFIG=~/.config/zellij/config.kdl

# ─────────────────────────────────────────────────────────────────────────────
# Clean (optional)
# ─────────────────────────────────────────────────────────────────────────────

if [[ "$1" == "--clean" ]]; then
    echo "Cleaning..."
    # Kill worktrunk sessions
    if command -v zellij &> /dev/null; then
        zellij list-sessions --no-formatting 2>/dev/null | grep '^wt:' | awk '{print $1}' | \
            while read -r session; do zellij kill-session "$session" 2>/dev/null || true; done
    fi
    # Clear plugin cache
    rm -f ~/Library/Caches/org.Zellij-Contributors.Zellij/*/[0-9]* 2>/dev/null || true
    rm -rf ~/.cache/zellij/plugins/* 2>/dev/null || true
    # Remove existing installation
    rm -f "$PLUGIN" "$LAYOUT" 2>/dev/null || true
    echo ""
fi

# ─────────────────────────────────────────────────────────────────────────────
# Build and install
# ─────────────────────────────────────────────────────────────────────────────

echo "Building and installing wt..."
cargo install --path=. --quiet

echo "Installing plugin..."
wt ui setup

# ─────────────────────────────────────────────────────────────────────────────
# Verify
# ─────────────────────────────────────────────────────────────────────────────

echo ""
wt ui status
