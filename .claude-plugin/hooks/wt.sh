#!/usr/bin/env bash
# Cross-platform wrapper for the worktrunk CLI.
# On Windows (MSYS/Cygwin), prefers git-wt.exe if available, then wt if it
# isn't the Windows Terminal alias (which lives in WindowsApps).
# On other platforms, uses wt.
# Usage: wt.sh [args...]

if [[ "$(uname -o 2>/dev/null)" =~ ^(Msys|Cygwin)$ ]]; then
    if command -v git-wt.exe >/dev/null 2>&1; then
        WT=git-wt.exe
    elif [[ "$(command -v wt 2>/dev/null)" != *WindowsApps* ]]; then
        WT=wt
    else
        echo "worktrunk: wt resolves to Windows Terminal; install git-wt or add worktrunk to PATH" >&2
        exit 1
    fi
else
    WT=wt
fi

"$WT" "$@"
"$WT" "$@"
