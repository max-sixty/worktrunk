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
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/max-sixty/worktrunk/releases/latest/download/worktrunk-installer.sh -o "$installer"
sh "$installer"

# Source the cargo env to pick up PATH changes from the installer.
# This handles custom CARGO_HOME and avoids hardcoding ~/.cargo/bin.
# POSIX sh exits the whole script when `.` can't read the file, even with
# `|| true`, so guard with an explicit existence check.
cargo_env="${CARGO_HOME:-$HOME/.cargo}/env"
if [ -r "$cargo_env" ]; then
    # shellcheck disable=SC1090
    . "$cargo_env"
fi

if ! command -v wt >/dev/null 2>&1; then
    echo ""
    echo "Warning: worktrunk installed but 'wt' not found in PATH."
    echo "Restart your shell and run 'wt config shell install' manually."
    exit 0
fi

# Configure shell integration. We use < /dev/tty so the interactive prompt
# works even when this script was piped into sh (e.g. curl ... | sh). On
# non-interactive contexts /dev/tty exists but isn't openable; probe before
# redirecting. Use `true` (a regular builtin) rather than `:` — POSIX exits
# the shell on a redirect failure against a special builtin.
if { true < /dev/tty; } 2>/dev/null; then
    wt config shell install < /dev/tty
else
    echo ""
    echo "Non-interactive environment detected."
    echo "Run 'wt config shell install' after restarting your shell."
fi
