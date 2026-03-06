#!/bin/sh
set -eu

# Worktrunk Installer (Unix)
# https://worktrunk.dev/install.sh

if [ "${OS:-}" = "Windows_NT" ]; then
    echo "Windows detected. Please use the PowerShell installer instead:"
    echo "  irm https://worktrunk.dev/install.ps1 | iex"
    exit 1
fi

echo "Installing worktrunk..."
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/max-sixty/worktrunk/releases/latest/download/worktrunk-installer.sh | sh || { echo "Installation failed."; exit 1; }

# cargo-dist installs to ~/.cargo/bin by default on Unix.
# We use < /dev/tty to ensure the interactive prompt can read from the terminal
# even when the script itself was piped into sh (e.g. curl ... | sh).
if [ -x "$HOME/.cargo/bin/wt" ]; then
    "$HOME/.cargo/bin/wt" config shell install < /dev/tty
elif command -v wt >/dev/null 2>&1; then
    wt config shell install < /dev/tty
else
    echo ""
    echo "Warning: worktrunk installed but 'wt' not found in PATH."
    echo "Please restart your shell and run 'wt config shell install' manually."
fi
