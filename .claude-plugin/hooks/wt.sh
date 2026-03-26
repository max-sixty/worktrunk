#!/usr/bin/env bash
# Cross-platform wrapper for the worktrunk CLI.
# On Windows (MSYS/Cygwin), prefers git-wt.exe if available, then wt if it
# isn't the Windows Terminal alias (which lives in WindowsApps).
# On other platforms, uses wt.
# Usage: wt.sh [args...]

# Resolve the worktrunk binary
if command -v git-wt.exe >/dev/null 2>&1; then
    WT=git-wt.exe
elif command -v wt >/dev/null 2>&1; then
    # On Windows, wt.exe in WindowsApps is Windows Terminal, not worktrunk
    if [[ "$(command -v wt)" == *WindowsApps* ]]; then
        echo "worktrunk: 'wt' resolves to Windows Terminal; install worktrunk as git-wt.exe or remove the Windows Terminal alias — see https://worktrunk.dev/worktrunk/#install" >&2
        exit 1
    fi
    WT=wt
else
    echo "worktrunk: could not find 'wt' in PATH" >&2
    exit 1
fi

"$WT" "$@"
