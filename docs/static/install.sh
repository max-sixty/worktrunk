#!/bin/sh
set -eu

# Worktrunk Installer (Unix)
# https://worktrunk.dev/install.sh

if [ "${OS:-}" = "Windows_NT" ]; then
    echo "Windows detected. Please use the PowerShell installer instead:"
    echo "  powershell -c \"irm https://worktrunk.dev/install.ps1 | iex\""
    exit 1
fi

echo "Installing worktrunk..."

# Download to a temp file instead of piping curl to sh. This avoids two issues:
# 1. Pipe swallows curl failures (pipefail is not POSIX)
# 2. Piping consumes stdin, blocking interactive prompts in the installer
installer="$(mktemp)"
trap 'rm -f "$installer"' EXIT
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/max-sixty/worktrunk/releases/latest/download/worktrunk-installer.sh -o "$installer" || {
    echo "Download failed."
    exit 1
}
sh "$installer" || {
    echo "Installation failed."
    exit 1
}

# Source the cargo env to pick up PATH changes from the installer.
# This handles custom CARGO_HOME and avoids hardcoding ~/.cargo/bin.
# shellcheck disable=SC1091
. "${CARGO_HOME:-$HOME/.cargo}/env" 2>/dev/null || true

if ! command -v wt >/dev/null 2>&1; then
    echo ""
    echo "Warning: worktrunk installed but 'wt' not found in PATH."
    echo "Restart your shell and run 'wt config shell install' manually."
    exit 0
fi

# Configure shell integration. We use < /dev/tty to ensure the interactive
# prompt can read from the terminal even when this script was piped into sh
# (e.g. curl ... | sh).
if [ -e /dev/tty ]; then
    wt config shell install < /dev/tty
else
    echo ""
    echo "Non-interactive environment detected."
    echo "Run 'wt config shell install' after restarting your shell."
fi
