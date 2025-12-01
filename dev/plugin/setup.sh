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
echo "Verifying..."
errors=0

# Plugin
if [[ -f "$PLUGIN" ]]; then
    echo "  OK  Plugin: $PLUGIN"
else
    echo "  ERR Plugin not found: $PLUGIN"
    errors=$((errors + 1))
fi

# Layout
if [[ -f "$LAYOUT" ]]; then
    echo "  OK  Layout: $LAYOUT"
else
    echo "  ERR Layout not found: $LAYOUT"
    errors=$((errors + 1))
fi

# Config
if [[ -f "$CONFIG" ]]; then
    if grep -q "wt-bridge.wasm" "$CONFIG"; then
        echo "  OK  Config: $CONFIG (has load_plugins)"
    else
        echo "  ERR Config missing load_plugins entry: $CONFIG"
        errors=$((errors + 1))
    fi
else
    echo "  ERR Config not found: $CONFIG"
    errors=$((errors + 1))
fi

# ─────────────────────────────────────────────────────────────────────────────
# Result
# ─────────────────────────────────────────────────────────────────────────────

echo ""
if [[ $errors -gt 0 ]]; then
    echo "Setup incomplete. Fix errors above and re-run."
    exit 1
fi

echo "Setup complete."
echo ""
echo "Next: Run 'wt ui' to enter workspace, grant permissions when prompted."
